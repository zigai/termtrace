#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${TERMTRACE_BINARY:-$root/target/debug/termtrace}"
readonly VERIFICATION_RANGE_FROM_NS=11000000000
readonly VERIFICATION_RANGE_TO_NS=12350000000
readonly VERIFICATION_RANGE_SUFFIX="range-${VERIFICATION_RANGE_FROM_NS}-${VERIFICATION_RANGE_TO_NS}"
readonly VERIFICATION_VIEWPORT_WIDTH=1280
readonly VERIFICATION_VIEWPORT_HEIGHT=720
readonly VERIFICATION_VIEWPORT="${VERIFICATION_VIEWPORT_WIDTH}x${VERIFICATION_VIEWPORT_HEIGHT}"
readonly VERIFICATION_FONT_SIZE=18

output=""
keep_log=false
verbose=false
validation_only=false
output_supplied=false

usage() {
    cat <<'EOF'
Run the native capture verification test.

Usage:
  just native-check

`just native-check` is the only public entrypoint. The script options below are for
automation and diagnostics.

Options:
  -o, --output PATH  Write evidence to PATH instead of a temporary directory.
      --validate PATH
                        Validate an existing verification recording without recapturing.
      --keep-log       Retain the detailed command log after a successful run.
  -v, --verbose        Stream raw build and Termtrace output to the console.
  -h, --help           Show this help.

The test builds Termtrace, records a timed color fixture, creates full and
ranged native captures, takes a strict point-in-time screenshot, and validates
the resulting manifests, media, replay range, and visual configuration.

Normal output is limited to eight progress steps and a final artifact summary.
Detailed command output is retained automatically when a step fails. Use
--validate to rerun the checks against retained evidence after fixing a check.
EOF
}

die() {
    printf 'error: %s\n' "$1" >&2
    exit 2
}

while (($#)); do
    case "$1" in
        -o | --output)
            (($# >= 2)) || die "$1 requires a path"
            output="$2"
            output_supplied=true
            shift 2
            ;;
        --validate)
            (($# >= 2)) || die "$1 requires a recording path"
            output="$2"
            output_supplied=true
            validation_only=true
            shift 2
            ;;
        --keep-log)
            keep_log=true
            shift
            ;;
        -v | --verbose)
            verbose=true
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

if [[ "$output_supplied" == true && -z "$output" ]]; then
    die "output path must not be empty"
fi

if [[ -z "$output" ]]; then
    temporary_root="$(mktemp -d)" || die "could not create a temporary output directory"
    output="$temporary_root/native-verification.termtrace"
fi

output_parent="$(dirname -- "$output")"
[[ -d "$output_parent" ]] || die "output parent does not exist: $output_parent"
output_parent="$(cd -- "$output_parent" && pwd -P)" ||
    die "could not resolve output parent: $output_parent"
output="$output_parent/$(basename -- "$output")"
readonly output

if [[ "$validation_only" == true ]]; then
    [[ -d "$output" ]] || die "verification recording does not exist: $output"
    log="${output}.validation.$$.log"
else
    [[ ! -e "$output" ]] || die "output already exists: $output"
    log="${output}.verification.log"
fi
readonly log

profile=""
current_step="Initializing verification test"
failed_check=""
failed_expected=""
failed_observed=""
failed_file=""
report_failure=false

capture_unexpected_error() {
    local status="$1"
    local line="$2"
    local command="$3"
    if [[ -z "$failed_check" ]]; then
        failed_check="unexpected script error"
        failed_expected="command completes successfully"
        failed_observed="exit status $status at line $line: $command"
        failed_file="$log"
    fi
    return "$status"
}

cleanup() {
    local status=$?
    trap - ERR EXIT
    set +e
    if [[ -n "$profile" ]]; then
        rm -f "$profile"
    fi
    if ((status != 0)) && [[ -z "$failed_check" ]]; then
        failed_check="unclassified script failure"
        failed_expected="verification step completes successfully"
        failed_observed="script exited with status $status during: $current_step"
        failed_file="$log"
    fi
    if ((status != 0)) && [[ -n "$failed_check" ]]; then
        printf '[failure] check=%s expected=%s observed=%s artifact=%s\n' \
            "$failed_check" "$failed_expected" "$failed_observed" "$failed_file" >>"$log"
    fi
    if ((status != 0)) && [[ "$report_failure" == true ]]; then
        printf '\n[FAIL] %s\n' "$current_step" >&2
        if [[ -n "$failed_check" ]]; then
            printf '  Check:    %s\n' "$failed_check" >&2
            printf '  Expected: %s\n' "$failed_expected" >&2
            printf '  Observed: %s\n' "$failed_observed" >&2
            if [[ -n "$failed_file" ]]; then
                printf '  Artifact: %s\n' "$failed_file" >&2
            fi
        fi
        printf 'Detailed log: %s\n' "$log" >&2
        printf 'Run again: just native-check\n' >&2
    elif ((status == 0)) && [[ "$keep_log" == false ]]; then
        rm -f "$log"
    fi
    exit "$status"
}
trap 'capture_unexpected_error "$?" "$LINENO" "$BASH_COMMAND"' ERR
trap cleanup EXIT

if ! (
    set -o noclobber
    : >"$log"
) 2>/dev/null; then
    die "diagnostic log already exists or could not be created: $log"
fi
report_failure=true
if [[ "$validation_only" == false ]]; then
    profile="$(mktemp)" || die "could not create the temporary Ghostty profile"
fi

clear_failed_check() {
    failed_check=""
    failed_expected=""
    failed_observed=""
    failed_file=""
}

require_commands() {
    local -a missing=()
    local command
    for command in "$@"; do
        if ! command -v "$command" >/dev/null 2>&1; then
            missing+=("$command")
        fi
    done
    if ((${#missing[@]} == 0)); then
        clear_failed_check
        return 0
    fi
    failed_check="required_commands"
    failed_expected="available commands: $*"
    failed_observed="missing commands: ${missing[*]}"
    failed_file="$log"
    return 2
}

run() {
    local status
    {
        printf '\n$'
        printf ' %q' "$@"
        printf '\n'
    } >>"$log"
    if [[ "$verbose" == true ]]; then
        if "$@" 2>&1 | tee -a "$log"; then
            clear_failed_check
            return 0
        else
            status=$?
        fi
    elif "$@" >>"$log" 2>&1; then
        clear_failed_check
        return 0
    else
        status=$?
    fi
    failed_check="command execution"
    failed_expected="exit status 0"
    failed_observed="exit status $status: $(printf '%q ' "$@")"
    failed_file="$log"
    printf '[command] failed with status %d\n' "$status" >>"$log"
    return "$status"
}

total_steps=8
step_number=0
step() {
    step_number=$((step_number + 1))
    current_step="$1"
    printf '[%d/%d] %s\n' "$step_number" "$total_steps" "$current_step"
}

check_jq() {
    local id="$1"
    local file="$2"
    local predicate="$3"
    local expected="$4"
    local observation="$5"
    local mode="${6:-single}"
    local status
    local observed
    local jq_predicate="$predicate"
    local jq_observation="$observation"
    local -a jq_options=(-s)
    if [[ "$mode" != "slurp" ]]; then
        jq_predicate="length == 1 and (.[0] | ($predicate))"
        jq_observation="if length == 1 then (.[0] | ($observation)) else {document_count: length} end"
    fi
    if [[ -L "$file" || ! -f "$file" || ! -s "$file" ]]; then
        failed_check="$id"
        failed_expected="$expected"
        if [[ -L "$file" ]]; then
            failed_observed="artifact is a symbolic link"
        elif [[ ! -e "$file" ]]; then
            failed_observed="artifact is missing"
        elif [[ ! -f "$file" ]]; then
            failed_observed="artifact is not a regular file"
        else
            failed_observed="artifact is empty"
        fi
        failed_file="$file"
        return 1
    fi
    if jq "${jq_options[@]}" -e "$jq_predicate" "$file" >/dev/null 2>>"$log"; then
        printf '[check:%s] pass\n' "$id" >>"$log"
        clear_failed_check
        return 0
    else
        status=$?
    fi
    if ! observed="$(jq "${jq_options[@]}" -c "$jq_observation" "$file" 2>>"$log")"; then
        observed="<artifact could not be summarized>"
    fi
    failed_check="$id"
    failed_expected="$expected"
    failed_observed="$observed (jq status $status)"
    failed_file="$file"
    printf '[check:%s] failed: expected %s; observed %s\n' \
        "$id" "$expected" "$observed" >>"$log"
    return 1
}

check_media_stream() {
    local id="$1"
    local file="$2"
    local codec="$3"
    local width="$4"
    local height="$5"
    local probe
    local status
    if probe="$(ffprobe -v error -select_streams v:0 \
        -show_entries stream=codec_name,width,height -of json "$file" 2>>"$log")"; then
        if jq -e --arg codec "$codec" --argjson width "$width" --argjson height "$height" '
          .streams | length == 1 and
          .[0].codec_name == $codec and
          .[0].width == $width and
          .[0].height == $height
        ' <<<"$probe" >/dev/null 2>>"$log"; then
            printf '[check:%s] pass\n' "$id" >>"$log"
            clear_failed_check
            return 0
        else
            status=$?
        fi
    else
        status=$?
    fi
    printf '[probe:%s] %s\n' "$id" "$probe" >>"$log"
    failed_check="$id"
    failed_expected="$codec media with dimensions ${width}x${height}"
    if [[ -n "$probe" ]]; then
        failed_observed="$(jq -c \
            '{streams: [.streams[]? | {codec_name, width, height}]}' \
            <<<"$probe" 2>>"$log" || printf '<invalid probe report>')"
    else
        failed_observed="FFprobe could not decode the artifact (status $status)"
    fi
    failed_file="$file"
    return 1
}

check_session_integrity() {
    local report
    local status
    local finding
    {
        printf '\n$ %q integrity verify --level content --json %q\n' \
            "$binary" "$output"
    } >>"$log"
    if [[ "$verbose" == true ]]; then
        if report="$("$binary" integrity verify \
            --level content --json "$output" 2> >(tee -a "$log" >&2))"; then
            status=0
        else
            status=$?
        fi
    elif report="$("$binary" integrity verify \
        --level content --json "$output" 2>>"$log")"; then
        status=0
    else
        status=$?
    fi
    printf '%s\n' "$report" >>"$log"
    if [[ "$verbose" == true ]]; then
        printf '%s\n' "$report"
    fi
    if [[ "$status" == 0 ]] &&
        jq -e '
          .valid == true and
          (.findings | length == 0) and
          .artifact_roots_verified >= 5
        ' <<<"$report" >/dev/null 2>>"$log"; then
        printf '[check:session_and_artifact_integrity] pass\n' >>"$log"
        clear_failed_check
        return 0
    fi
    if ! finding="$(jq -r '
      if (.findings | length) > 0 then
        .findings[0] | "\(.code): \(.message)"
      else
        "verification report was invalid or incomplete"
      end
    ' <<<"$report" 2>>"$log")"; then
        finding="verification command did not return a valid report"
    fi
    failed_check="session_and_artifact_integrity"
    failed_expected="content-valid canonical recording and at least five verified verification artifact roots"
    failed_observed="$finding; artifact_roots_verified=$(jq -r '.artifact_roots_verified // "unknown"' <<<"$report" 2>>"$log") (verification status $status)"
    failed_file="$output"
    return 1
}

check_contains() {
    local id="$1"
    local file="$2"
    local text="$3"
    local status
    if grep -Fq "$text" "$file" 2>>"$log"; then
        printf '[check:%s] pass\n' "$id" >>"$log"
        clear_failed_check
        return 0
    else
        status=$?
    fi
    failed_check="$id"
    failed_expected="file contains: $text"
    failed_observed="$([[ "$status" == 1 ]] && printf 'text was absent' || printf 'grep failed with status %s' "$status")"
    failed_file="$file"
    return 1
}

check_absent_regex() {
    local id="$1"
    local file="$2"
    local pattern="$3"
    local status
    if grep -Eq "$pattern" "$file" 2>>"$log"; then
        failed_check="$id"
        failed_expected="no text matching: $pattern"
        failed_observed="forbidden text was present"
        failed_file="$file"
        return 1
    else
        status=$?
    fi
    if [[ "$status" == 1 ]]; then
        printf '[check:%s] pass\n' "$id" >>"$log"
        clear_failed_check
        return 0
    fi
    failed_check="$id"
    failed_expected="a readable file with no text matching: $pattern"
    failed_observed="grep failed with status $status"
    failed_file="$file"
    return 1
}

check_nonempty_files() {
    local id="$1"
    shift
    local file
    for file in "$@"; do
        if [[ -L "$file" || ! -f "$file" || ! -s "$file" ]]; then
            failed_check="$id"
            failed_expected="every required artifact is a non-empty regular file"
            if [[ -L "$file" ]]; then
                failed_observed="artifact is a symbolic link"
            elif [[ ! -e "$file" ]]; then
                failed_observed="artifact is missing"
            elif [[ ! -f "$file" ]]; then
                failed_observed="artifact is not a regular file"
            else
                failed_observed="artifact is empty"
            fi
            failed_file="$file"
            return 1
        fi
    done
    printf '[check:%s] pass\n' "$id" >>"$log"
    clear_failed_check
}

check_missing() {
    local id="$1"
    local file="$2"
    if [[ ! -e "$file" && ! -L "$file" ]]; then
        printf '[check:%s] pass\n' "$id" >>"$log"
        clear_failed_check
        return 0
    fi
    failed_check="$id"
    failed_expected="artifact is absent"
    failed_observed="artifact exists"
    failed_file="$file"
    return 1
}

check_equal() {
    local id="$1"
    local expected="$2"
    local observed="$3"
    local file="$4"
    if [[ "$expected" == "$observed" ]]; then
        printf '[check:%s] pass\n' "$id" >>"$log"
        clear_failed_check
        return 0
    fi
    failed_check="$id"
    failed_expected="$expected"
    failed_observed="$observed"
    failed_file="$file"
    return 1
}

check_less_than() {
    local id="$1"
    local left="$2"
    local right="$3"
    local file="$4"
    if awk -v left="$left" -v right="$right" \
        'BEGIN { exit !(left < right) }' >>"$log" 2>&1; then
        printf '[check:%s] pass\n' "$id" >>"$log"
        clear_failed_check
        return 0
    fi
    failed_check="$id"
    failed_expected="$left < $right"
    failed_observed="$left >= $right"
    failed_file="$file"
    return 1
}

check_pixels_differ() {
    local id="$1"
    local left="$2"
    local right="$3"
    local status pixels
    pixels="$(mktemp -d)" || return 1
    if ! ffmpeg -v error -i "$left" -frames:v 1 -f rawvideo -pix_fmt rgb24 \
        "$pixels/left.rgb" >>"$log" 2>&1 ||
        ! ffmpeg -v error -i "$right" -frames:v 1 -f rawvideo -pix_fmt rgb24 \
            "$pixels/right.rgb" >>"$log" 2>&1; then
        rm -rf "$pixels"
        failed_check="$id"
        failed_expected="two decodable screenshots"
        failed_observed="screenshot pixel decoding failed"
        failed_file="$left and $right"
        return 1
    fi
    if cmp -s "$pixels/left.rgb" "$pixels/right.rgb"; then
        status=0
    else
        status=$?
    fi
    rm -rf "$pixels"
    if [[ "$status" == 1 ]]; then
        printf '[check:%s] pass\n' "$id" >>"$log"
        clear_failed_check
        return 0
    fi
    failed_check="$id"
    failed_expected="screenshots have different decoded pixels"
    if [[ "$status" == 0 ]]; then
        failed_observed="screenshots have identical decoded pixels"
    else
        failed_observed="pixel comparison failed with status $status"
    fi
    failed_file="$left and $right"
    return 1
}

validate_verification_artifacts() {
    local range_derived="$output/derived-$VERIFICATION_RANGE_SUFFIX"
    local range_visual="$output/visual-$VERIFICATION_RANGE_SUFFIX"
    local manifest="$output/visual/manifest.json"
    local range_manifest="$range_visual/manifest.json"
    local full_duration
    local range_duration
    local session_hash
    local range_duration_with_margin
    local range_visual_hash
    local visual_hash

    check_jq "full_manifest_contract" "$manifest" '
      .version == 5 and
      (.source_event_hash | type == "string" and test("^[0-9a-f]{64}$")) and
      (.effective_config_hash | type == "string" and test("^[0-9a-f]{64}$")) and
      .fps == 24 and
      .source_range == null and
      .width == 1280 and
      .height == 720 and
      .capture_kind == "video" and
      .video_probe.streams[0].codec_name == "h264" and
      .video_probe.streams[0].width == 1280 and
      .video_probe.streams[0].height == 720 and
      .video_probe.streams[0].avg_frame_rate == "24/1" and
      (.video_probe.streams[0].nb_read_packets | tonumber) > 0 and
      .screenshot_probe.streams[0].codec_name == "png" and
      .screenshot_probe.streams[0].width == 1280 and
      .screenshot_probe.streams[0].height == 720
    ' "manifest v5; source/config hashes; 1280x720 H.264 at 24 FPS; non-empty packets; PNG screenshot" \
        '{version, source_event_hash, effective_config_hash, fps, source_range,
        width, height, capture_kind,
        video_codec: .video_probe.streams[0].codec_name,
        video_width: .video_probe.streams[0].width,
        video_height: .video_probe.streams[0].height,
        frame_rate: .video_probe.streams[0].avg_frame_rate,
        packets: .video_probe.streams[0].nb_read_packets,
        screenshot_codec: .screenshot_probe.streams[0].codec_name,
        screenshot_width: .screenshot_probe.streams[0].width,
        screenshot_height: .screenshot_probe.streams[0].height}'

    check_jq "full_video_map_contract" "$output/visual/video-map.jsonl" '
      length > 0 and
      all(.[];
        type == "object" and
        (.state_id | type == "number" and . >= 0 and floor == .) and
        (.state_hash | type == "string" and test("^[0-9a-f]{64}$")) and
        (.monotonic_ns | type == "number" and . >= 0 and floor == .) and
        (.frame | type == "number" and . >= 0 and floor == .) and
        (.presented | type == "boolean")) and
      ([.[].monotonic_ns] == ([.[].monotonic_ns] | sort)) and
      ([.[].frame] == ([.[].frame] | sort)) and
      any(.[]; .presented)
    ' "ordered, schema-complete full-video map with at least one presented frame" \
        '{count: length,
        first: (.[0] // null),
        last: (.[-1] // null),
        presented: ([.[] | select(.presented == true)] | length)}' slurp

    check_jq "range_manifest_contract" "$range_manifest" '
      .version == 5 and
      (.source_event_hash | type == "string" and test("^[0-9a-f]{64}$")) and
      (.effective_config_hash | type == "string" and test("^[0-9a-f]{64}$")) and
      .source_range.start_monotonic_ns == 11000000000 and
      .source_range.end_monotonic_ns == 12350000000 and
      .fps == 24 and
      .width == 1280 and
      .height == 720 and
      .capture_kind == "video" and
      .video_probe.streams[0].codec_name == "h264" and
      .video_probe.streams[0].width == 1280 and
      .video_probe.streams[0].height == 720 and
      .video_probe.streams[0].avg_frame_rate == "24/1" and
      (.video_probe.streams[0].nb_read_packets | tonumber) > 0 and
      .screenshot_probe.streams[0].codec_name == "png" and
      .screenshot_probe.streams[0].width == 1280 and
      .screenshot_probe.streams[0].height == 720
    ' "manifest v5; range [11000000000,12350000000); source/config hashes; 1280x720 H.264 at 24 FPS; non-empty packets; PNG screenshot" \
        '{version, source_event_hash, effective_config_hash, source_range, fps,
        width, height, capture_kind,
        video_codec: .video_probe.streams[0].codec_name,
        video_width: .video_probe.streams[0].width,
        video_height: .video_probe.streams[0].height,
        frame_rate: .video_probe.streams[0].avg_frame_rate,
        packets: .video_probe.streams[0].nb_read_packets,
        screenshot_codec: .screenshot_probe.streams[0].codec_name,
        screenshot_width: .screenshot_probe.streams[0].width,
        screenshot_height: .screenshot_probe.streams[0].height}'

    check_jq "range_video_map_contract" "$range_visual/video-map.jsonl" '
      length > 0 and
      all(.[];
        type == "object" and
        (.state_id | type == "number" and . >= 0 and floor == .) and
        (.state_hash | type == "string" and test("^[0-9a-f]{64}$")) and
        (.monotonic_ns | type == "number" and
          . >= 11000000000 and . < 12350000000 and floor == .) and
        (.frame | type == "number" and . >= 0 and floor == .) and
        (.presented | type == "boolean")) and
      ([.[].monotonic_ns] == ([.[].monotonic_ns] | sort)) and
      ([.[].frame] == ([.[].frame] | sort)) and
      any(.[]; .presented)
    ' "ordered, schema-complete map entries in [11000000000,12350000000) with at least one presented frame" \
        '{count: length,
        first: (.[0] // null),
        last: (.[-1] // null),
        presented: ([.[] | select(.presented == true)] | length)}' slurp

    check_jq "range_derived_contract" "$range_derived/manifest.json" '
      .source_range.start_monotonic_ns == 11000000000 and
      .source_range.end_monotonic_ns == 12350000000
    ' "derived source range [11000000000,12350000000)" '.source_range'

    check_contains "range_retains_preroll" "$range_derived/final.txt" "Long preroll"
    check_contains "range_contains_selection" "$range_derived/final.txt" "Selected range"
    check_absent_regex "range_excludes_tail" "$range_derived/final.txt" "excluded tail"

    if ! full_duration="$(jq -er \
        '.video_probe.format.duration | tonumber | select(. > 0)' \
        "$manifest" 2>>"$log")"; then
        failed_check="full_video_duration"
        failed_expected="positive numeric full video duration"
        failed_observed="duration was absent, invalid, or non-positive"
        failed_file="$manifest"
        return 1
    fi
    if ! range_duration="$(jq -er \
        '.video_probe.format.duration | tonumber | select(. > 0)' \
        "$range_manifest" 2>>"$log")"; then
        failed_check="range_video_duration"
        failed_expected="positive numeric ranged video duration"
        failed_observed="duration was absent, invalid, or non-positive"
        failed_file="$range_manifest"
        return 1
    fi
    if ! range_duration_with_margin="$(awk -v range="$range_duration" \
        'BEGIN { printf "%.9f", range + 5 }' 2>>"$log")"; then
        failed_check="range_video_duration_margin"
        failed_expected="numeric ranged video duration plus five seconds"
        failed_observed="duration margin could not be computed"
        failed_file="$range_manifest"
        return 1
    fi
    check_less_than "range_video_duration_margin" "$range_duration_with_margin" \
        "$full_duration" "$range_manifest"

    check_jq "session_manifest_contract" "$output/session.json" '
      .version == 12 and
      .ghostty_visual.source_config != null and
      .ghostty_visual.overrides.font_size == "18" and
      .ghostty_visual.overrides.padding_x == 6 and
      .ghostty_visual.overrides.padding_y == 4 and
      .ghostty_visual.overrides.foreground == "#e6e6e6" and
      .ghostty_visual.overrides.background == "#101010" and
      .ghostty_visual.replay_profile == "ghostty-profile.ghostty" and
      (.ghostty_visual.replay_profile_hash |
        type == "string" and test("^[0-9a-f]{64}$")) and
      (.ghostty_visual.effective_config_hash |
        type == "string" and test("^[0-9a-f]{64}$"))
    ' "session manifest v12 with the requested visual overrides and 64-character hashes" \
        '{version, source_config: (.ghostty_visual.source_config != null),
        overrides: .ghostty_visual.overrides,
        replay_profile: .ghostty_visual.replay_profile,
        replay_profile_hash: .ghostty_visual.replay_profile_hash,
        effective_config_hash: .ghostty_visual.effective_config_hash}'

    if ! session_hash="$(jq -er '.ghostty_visual.effective_config_hash' \
        "$output/session.json" 2>>"$log")"; then
        failed_check="session_effective_config_hash"
        failed_expected="session effective configuration hash"
        failed_observed="hash was absent or invalid"
        failed_file="$output/session.json"
        return 1
    fi
    if ! visual_hash="$(jq -er '.effective_config_hash' "$manifest" 2>>"$log")"; then
        failed_check="capture_effective_config_hash"
        failed_expected="capture effective configuration hash"
        failed_observed="hash was absent or invalid"
        failed_file="$manifest"
        return 1
    fi
    check_equal "effective_config_hash_matches" "$session_hash" "$visual_hash" "$manifest"
    if ! range_visual_hash="$(jq -er '.effective_config_hash' \
        "$range_manifest" 2>>"$log")"; then
        failed_check="range_capture_effective_config_hash"
        failed_expected="ranged capture effective configuration hash"
        failed_observed="hash was absent or invalid"
        failed_file="$range_manifest"
        return 1
    fi
    check_equal "range_effective_config_hash_matches" \
        "$session_hash" "$range_visual_hash" "$range_manifest"

    check_nonempty_files "profile_and_full_capture_artifacts" \
        "$output/ghostty-profile.ghostty" \
        "$output/visual/ghostty-effective-config.txt" \
        "$output/visual/session.mkv" \
        "$output/visual/final.png" \
        "$output/visual/video-map.jsonl"
    check_media_stream "full_video_decodes" \
        "$output/visual/session.mkv" h264 "$VERIFICATION_VIEWPORT_WIDTH" "$VERIFICATION_VIEWPORT_HEIGHT"
    check_media_stream "full_screenshot_decodes" \
        "$output/visual/final.png" png "$VERIFICATION_VIEWPORT_WIDTH" "$VERIFICATION_VIEWPORT_HEIGHT"
    check_absent_regex "profile_has_no_external_config_paths" \
        "$output/ghostty-profile.ghostty" \
        '^[[:space:]]*config-(file|default-files)[[:space:]]*='

    check_nonempty_files "point_screenshot_artifacts" \
        "$output/native-single/final.png" \
        "$output/native-single/ghostty-effective-config.txt"
    check_media_stream "point_screenshot_decodes" \
        "$output/native-single/final.png" png "$VERIFICATION_VIEWPORT_WIDTH" "$VERIFICATION_VIEWPORT_HEIGHT"
    check_missing "point_screenshot_has_no_video" "$output/native-single/session.mkv"
    check_jq "point_screenshot_manifest_contract" \
        "$output/native-single/manifest.json" '
      .version == 5 and
      .source_range == null and
      .width == 1280 and
      .height == 720 and
      (.effective_config_hash |
        type == "string" and test("^[0-9a-f]{64}$")) and
      .capture_kind == "screenshot" and
      .presentation_evidence == "post_endpoint_stable_capture" and
      .guarantee_level == "native_capture_environment" and
      (.selected_state_id | type == "number" and . >= 0 and floor == .) and
      (.selected_state_hash |
        type == "string" and test("^[0-9a-f]{64}$")) and
      (.endpoint_event_seq | type == "number" and . >= 0 and floor == .) and
      (.endpoint_event_byte_offset | type == "number" and . >= 0 and floor == .) and
      (.frame_samples | length) >= 3 and
      all(.frame_samples[];
        (.sample | type == "number" and . >= 0 and floor == .) and
        (.captured_unix_ns | type == "number" and . >= 0 and floor == .) and
        (.blake3 | type == "string" and test("^[0-9a-f]{64}$")) and
        (.same_as_previous | type == "boolean")) and
      .frame_samples[-1].same_as_previous == true and
      .screenshot_probe.streams[0].codec_name == "png" and
      .screenshot_probe.streams[0].width == 1280 and
      .screenshot_probe.streams[0].height == 720 and
      .video_probe == null
    ' "manifest v5 with stable 1280x720 native screenshot evidence, typed state/endpoint fields, and at least three valid frame samples" \
        '{version, source_range, width, height, effective_config_hash,
        capture_kind, presentation_evidence, guarantee_level,
        selected_state_id, selected_state_hash,
        endpoint_event_seq, endpoint_event_byte_offset,
        frame_sample_count: (.frame_samples | length),
        final_sample_stable: .frame_samples[-1].same_as_previous,
        screenshot_codec: .screenshot_probe.streams[0].codec_name,
        video_probe}'

    check_nonempty_files "range_capture_artifacts" \
        "$range_visual/session.mkv" \
        "$range_visual/final.png" \
        "$range_visual/ghostty-effective-config.txt" \
        "$range_visual/video-map.jsonl"
    check_media_stream "range_video_decodes" \
        "$range_visual/session.mkv" h264 "$VERIFICATION_VIEWPORT_WIDTH" "$VERIFICATION_VIEWPORT_HEIGHT"
    check_media_stream "range_screenshot_decodes" \
        "$range_visual/final.png" png "$VERIFICATION_VIEWPORT_WIDTH" "$VERIFICATION_VIEWPORT_HEIGHT"
    check_jq "artifact_root_catalog_contract" "$output/artifact-roots.jsonl" '
      (map(.artifact_directory)) as $directories |
      length >= 5 and
      ([
        "derived",
        "visual",
        "native-single",
        "derived-range-11000000000-12350000000",
        "visual-range-11000000000-12350000000"
      ] - $directories | length == 0)
    ' "catalog includes the five expected full, ranged, and point-capture artifact roots" \
        '{count: length, directories: map(.artifact_directory)}' slurp
    check_pixels_differ "full_and_range_screenshots_differ" \
        "$output/visual/final.png" "$range_visual/final.png"
    check_session_integrity
}

if [[ "$validation_only" == true ]]; then
    total_steps=2
    printf 'Termtrace native capture validation\n'
    printf '  Recording: %s\n\n' "$output"
    step "Checking validation tools"
    require_commands jq awk grep cmp ffprobe
    if [[ ! -x "$binary" ]]; then
        failed_check="termtrace_binary"
        failed_expected="an executable Termtrace binary; set TERMTRACE_BINARY or run cargo build"
        failed_observed="binary is missing or not executable"
        failed_file="$binary"
        exit 2
    fi
    step "Validating retained manifests, media, range boundaries, and provenance"
    validate_verification_artifacts
    printf '\n[PASS] Retained native capture evidence is valid.\n'
    if [[ "$keep_log" == true ]]; then
        printf 'Detailed log: %s\n' "$log"
    fi
    exit 0
fi

printf 'Termtrace native capture verification test\n'
printf '  Output:  %s\n\n' "$output"

step "Checking host commands"
required_commands=(cargo jq awk grep cmp ffprobe python3)
if [[ "$verbose" == true ]]; then
    required_commands+=(tee)
fi
require_commands "${required_commands[@]}"

cd "$root"

if [[ -z "${TERMTRACE_BINARY:-}" ]]; then
    step "Building Termtrace"
    run cargo build
else
    [[ -x "$binary" ]] || die "TERMTRACE_BINARY is not executable: $binary"
fi

step "Checking native capture capabilities"
run "$binary" doctor --json \
    --require native-screenshot,native-video

printf 'font-size = 11\nwindow-padding-x = 2\nbackground = #220000\n' >"$profile"

step "Recording the timed fixture (about 13 seconds)"
run "$binary" record --no-input --geometry fit \
    --viewport "$VERIFICATION_VIEWPORT" --ghostty-config "$profile" --font-size "$VERIFICATION_FONT_SIZE" \
    --padding-x 6 --padding-y 4 --foreground '#e6e6e6' \
    --background '#101010' --out "$output" -- /bin/sh -c \
    'printf "\033[2J\033[H\033[38;2;255;0;0mLong preroll\033[0m\n"; sleep 12; printf "\033[38;2;0;255;0mSelected range\033[0m\n"; sleep 0.5; printf "excluded tail\n"'
rm -f "$profile"

step "Indexing the complete recording"
run "$binary" analyze index "$output"

step "Capturing full video and a strict screenshot"
run "$binary" artifact capture "$output" --fps 24
run "$binary" screenshot "$output" \
    --renderer native --at 12.2s --viewport "$VERIFICATION_VIEWPORT" --strict \
    --out "$output/native-single"

readonly capture_range_visual="$output/visual-$VERIFICATION_RANGE_SUFFIX"

step "Capturing the selected 11.0s–12.35s range"
run "$binary" analyze index "$output" \
    --from "${VERIFICATION_RANGE_FROM_NS}ns" --to "${VERIFICATION_RANGE_TO_NS}ns"
range_capture_started="$(python3 -c 'import time; print(time.monotonic_ns())')"
readonly range_capture_started
run "$binary" artifact capture "$output" --fps 24 \
    --from "${VERIFICATION_RANGE_FROM_NS}ns" --to "${VERIFICATION_RANGE_TO_NS}ns"
range_capture_finished="$(python3 -c 'import time; print(time.monotonic_ns())')"
readonly range_capture_finished
range_capture_elapsed_ns="$((range_capture_finished - range_capture_started))"
readonly range_capture_elapsed_ns
check_less_than "range_capture_fast_forwards_preroll" \
    "$range_capture_elapsed_ns" "$VERIFICATION_RANGE_FROM_NS" "$capture_range_visual"

step "Validating manifests, media, range boundaries, and provenance"
validate_verification_artifacts

printf '\n[PASS] Native capture workflow is healthy.\n'
printf 'Evidence:\n'
printf '  Recording:        %s\n' "$output"
printf '  Full video:       %s\n' "$output/visual/session.mkv"
printf '  Full screenshot:  %s\n' "$output/visual/final.png"
printf '  Point screenshot: %s\n' "$output/native-single/final.png"
printf '  Ranged capture:   %s\n' "$capture_range_visual"
awk -v elapsed="$range_capture_elapsed_ns" -v preroll="$VERIFICATION_RANGE_FROM_NS" \
    'BEGIN { printf "Performance: ranged capture took %.3fs for %.3fs of pre-roll\n", elapsed / 1000000000, preroll / 1000000000 }'
if [[ "$keep_log" == true ]]; then
    printf 'Detailed log:      %s\n' "$log"
fi

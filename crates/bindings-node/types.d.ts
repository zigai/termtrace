/** SDK model types generated from the Rust type catalog. Do not edit; run `just generate`. */
import type { ScreenFrame as FrameHandle } from './index'
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
/** A native method result: the inner model type. */
export type Model<T> = T
/** A native method argument: the inner model type. */
export type Arg<T> = T

/** Complete result of one actor-ordered action. */
export interface ActOutcome {
  /** Actor sequence boundaries enclosing the action. */
  boundaries: ActorBoundaries
  /** Ordered input application outcome. */
  input: ActionInputResult
  /** Postcondition result exactly as the actor reported it. */
  postcondition: ActionPostconditionResult
  /** Identity of the action request. */
  request_id: string
}

/** Exact applied-input component of an actor-ordered action. */
export interface ActionInputResult {
  /** Number of atoms applied completely. */
  atom_count: number
  /** Optional per-atom outcomes requested by an explicit verbose caller. */
  atoms?: Array<InputSequenceAtomOutcome>
  /** Total exact PTY bytes written across every atom. */
  bytes_written: bigint
  /** First canonical input-intent event in the sequence. */
  first_intent_event_seq: bigint
  /** Last canonical input-result event in the sequence. */
  last_result_event_seq: bigint
  /** Requested delay between atoms. */
  pace_ns: bigint
  /** Canonical sequence identifier (the action request ID). */
  sequence_id: string
}

/** One changed ANSI or DEC terminal mode. */
export interface ActionModeChange {
  /** Later set/reset value. */
  after: boolean
  /** `true` for ANSI and `false` for DEC-private mode. */
  ansi: boolean
  /** Earlier set/reset value. */
  before: boolean
  /** Numeric terminal mode identifier. */
  number: number
}

/** Aggregate child output attributable to the last action boundary. */
export interface ActionOutputSummary {
  /** Exact output byte count without the output bytes themselves. */
  byte_count: bigint
  /** Canonical output-event count after the input intent. */
  event_count: bigint
}

/** Exact postcondition result returned without releasing actor ordering. */
export type ActionPostconditionResult = ActionPostconditionResultNone | ActionPostconditionResultIdle | ActionPostconditionResultScreen

/** Final state of an idle postcondition. */
export interface ActionPostconditionResultIdle {
  /** Exact terminating frame. */
  frame?: ScreenFrame | null
  kind: "idle"
  /** Whether the state remained unchanged for the requested interval. */
  matched: boolean
  /** Typed stability quality. */
  quality: StableCaptureQuality
  /** Exact terminating state hash when known. */
  state_hash?: string | null
}

/** No presented-state postcondition was requested. */
export interface ActionPostconditionResultNone {
  /** Exact immediate post-input frame. */
  frame?: ScreenFrame | null
  kind: "none"
  /** Always true after complete input application. */
  matched: boolean
  /** Exact terminating state hash when known. */
  state_hash?: string | null
}

/** Final state of a screen postcondition. */
export interface ActionPostconditionResultScreen {
  /** Exact terminating frame. */
  frame?: ScreenFrame | null
  kind: "screen"
  /** Whether the predicate matched before termination. */
  matched: boolean
  /** Closed completion classification. */
  quality: ScreenPostconditionQuality
  /** Exact terminating state hash when known. */
  state_hash?: string | null
}

/** First shell command that OSC 133 marks report after the action's input intent. */
export interface ActionShellCommand {
  /** Decoded command line when the shell sent one. */
  command?: Base64Bytes | null
  /** Mark where the command finished, absent while it still runs. */
  ended?: CommandMark | null
  /** Exit code the shell reported with the end mark. */
  exit_code?: number | null
  /** Mark where the command started running, absent when the shell skipped it. */
  started?: CommandMark | null
}

/** One bounded warning derived from canonical action evidence. */
export interface ActionWarning {
  /** Stable warning code. */
  code: string
  /** Conservative confidence label. */
  confidence: FindingConfidence
  /** Bounded neutral evidence facts. */
  facts: Array<string>
}

/** Durable and latest actor-processed positions returned by live operations. */
export interface ActorBoundaries {
  /** Latest boundary whose file synchronization completed successfully. */
  durable: CanonicalBoundary
  /** Latest boundary processed by the session actor, possibly volatile. */
  observed: CanonicalBoundary
}

/** Stable annotation anchor vocabulary. */
export type AnnotationAnchor = "top_left" | "top_right" | "bottom_left" | "bottom_right" | "center"

/** Named anchor plus signed pixel offset. */
export interface AnnotationPosition {
  /** Named canvas anchor. */
  anchor: AnnotationAnchor
  /** Signed horizontal offset from the anchor. */
  x: number
  /** Signed vertical offset from the anchor. */
  y: number
}

/** Annotation lifetime on the normalized presentation timeline. */
export type AnnotationScope = AnnotationScopeWholeVideo | AnnotationScopeClip | AnnotationScopeOutputFrame | AnnotationScopeRange

/** One normalized clip. */
export interface AnnotationScopeClip {
  /** Zero-based caller clip index. */
  clip_index: bigint
  scope: "clip"
}

/** One exact output frame. */
export interface AnnotationScopeOutputFrame {
  /** Zero-based presentation frame. */
  output_frame: bigint
  scope: "output_frame"
}

/** A source-selector-bounded half-open interval. */
export interface AnnotationScopeRange {
  /** Inclusive source endpoint. */
  from: EditSelector
  scope: "range"
  /** Exclusive source endpoint. */
  to: EditSelector
}

/** Every output frame. */
export interface AnnotationScopeWholeVideo {
  scope: "whole_video"
}

/** Deterministic visible text style compiled to ASS. */
export interface AnnotationStyle {
  /** `#RRGGBB` background. */
  background: string
  /** Background opacity from zero through 255. */
  background_opacity: number
  /** Border width in pixels. */
  border: number
  /** Font size in pixels. */
  font_size: number
  /** `#RRGGBB` foreground. */
  foreground: string
}

export type AnyValue = JsonValue

/** Successful acknowledged SDK instrumentation emission. This is not a persisted event envelope. */
export interface ApplicationAcknowledgement {
  /** True only after the actor's positive acknowledgement. */
  acknowledged: boolean
  /** Actor-observed and durable positions from the actual acknowledgement. */
  boundaries: ActorBoundaries
  /** Emitting process-instance identity. */
  process_instance_id: string
  /** Span identity for start/end, absent for a discrete event. */
  span_id?: string | null
}

/** Caller-reported process and clock information, not independently verified identity. The enclosing canonical event supplies recorder time and session identity. A process instance UUID distinguishes PID reuse. Span identifiers are scoped to that instance. */
export interface ApplicationEvent {
  /** Sensitive application data, excluded from metadata-only exports. */
  attributes: Record<string, JsonValue>
  /** Event or span lifecycle transition. */
  kind: ApplicationEventKind
  /** Caller-reported monotonic nanoseconds since instrumentation initialization. */
  monotonic_ns: bigint
  /** Diagnostic name, from 1 to 256 UTF-8 bytes. */
  name: string
  /** Optional parent span in the same process instance. */
  parent_span_id?: string | null
  /** Caller-reported operating-system PID. */
  process_id: number
  /** Stable UUID for the emitting process instance. */
  process_instance_id: string
  /** Span identity; required for span lifecycle transitions. */
  span_id?: string | null
}

/** Application instrumentation lifecycle. */
export type ApplicationEventKind = "event" | "span_start" | "span_end"

/** Exact manifest reference, relative to the artifact directory. */
export interface ArtifactManifestRef {
  /** BLAKE3 of the exact manifest bytes. */
  blake3: string
  /** Normalized relative filename. */
  path: ArtifactRelativePath
}

/** Whole-file integrity metadata embedded in a derived manifest. */
export interface ArtifactMember {
  /** BLAKE3 of the complete file. */
  blake3: string
  /** Exact byte length. */
  byte_length: bigint
  /** Stable media type. */
  media_type: string
  /** Normalized relative path below the artifact directory. */
  path: string
  /** Relationship to the derivation. */
  relationship: string
  /** Public schema identifier, when the file is structured data. */
  schema_type?: string | null
  /** Sensitivity classification. */
  sensitivity: ArtifactSensitivity
}

/** Explicit output kind, including previews rather than inferring from extensions. */
export type ArtifactProductKind = "snapshot" | "video" | "playback" | "presentation" | "preview"

/** Product provenance, with presentation explicitly retaining its source truth. */
export type ArtifactProvenance = ArtifactProvenanceSemanticRendering | ArtifactProvenanceNativeReplay | ArtifactProvenanceOriginalExecution | ArtifactProvenancePresentationOutput

/** Native replay pixels, not original execution. */
export interface ArtifactProvenanceNativeReplay {
  kind: "native_replay"
}

/** Original execution capture. */
export interface ArtifactProvenanceOriginalExecution {
  kind: "original_execution"
}

/** Derived presentation; authoritative mapping remains in its manifest. */
export interface ArtifactProvenancePresentationOutput {
  kind: "presentation_output"
  /** Truth of the input to presentation. */
  source_kind: ArtifactSourceKind
}

/** Semantic reconstruction, not original pixels. */
export interface ArtifactProvenanceSemanticRendering {
  kind: "semantic_rendering"
}

export type ArtifactRelativePath = string

/** Sensitivity classification for one derived artifact member. */
export type ArtifactSensitivity = "sensitive_terminal_data" | "operational_metadata"

/** Non-derived source truth; not a replacement for manifest truth-layer strings. */
export type ArtifactSourceKind = "semantic_rendering" | "native_replay" | "original_execution"

/** Binding to canonical source; no frame-exact original-capture correlation implied. */
export interface ArtifactSourceRef {
  /** Exact canonical prefix (header hash at sequence zero). */
  boundary: SdkCanonicalBoundary
  /** Canonical session UUID. */
  session_id: string
  /** Hash of the source session manifest. */
  session_manifest_hash: string
}

/** Stable frame identity included in bounded assertion diagnostics. */
export interface AssertionFrameIdentity {
  /** Whether the actor marked the state presentation eligible. */
  presented: boolean
  /** Exact rich projection hash. */
  projection_hash: string
  /** Exact semantic-state hash. */
  state_hash: string
}

/** One input atom in host form: `{"text"}`, `{"bytes"}`, `{"key"}`, or `{"paste"}`. */
export type AtomSpec = AtomSpecText | AtomSpecBytes | AtomSpecKey | AtomSpecPaste

/** Exact raw bytes. */
export interface AtomSpecBytes {
  bytes: Base64Bytes
}

/** One key chord in the shared grammar. */
export interface AtomSpecKey {
  key: string
}

/** Bracketed-paste text. */
export interface AtomSpecPaste {
  paste: string
}

/** UTF-8 text. */
export interface AtomSpecText {
  text: string
}

export type Base64Bytes = Uint8Array

/** Top-level whole-file bundle manifest. */
export interface BundleManifest {
  /** Exact source session path bytes. */
  canonical_session: Base64Bytes
  /** Stable manifest format. */
  format: string
  /** Whether canonical raw evidence is included. */
  includes_canonical_evidence: boolean
  /** Whole-file inventory; unmanifested payloads are invalid. */
  members: Array<ArtifactMember>
  /** Selected policy. */
  profile: BundleProfile
  /** Immutable canonical session identity. */
  session_id: string
  /** Final canonical event hash. */
  source_final_event_hash: string
  /** Manifest schema version. */
  version: number
}

/** Fixed bundle content policy. */
export type BundleProfile = "screen" | "debug" | "forensic"

/** Bounded exact PTY bytes; this never promises no future output will arrive. */
export interface ByteOutputPage {
  /** Exact PTY bytes, including NUL and invalid UTF-8. */
  bytes: Base64Bytes
  /** Checked resume position. */
  cursor: OutputPageCursor
  /** At least one further output byte is present in this snapshot. */
  has_more: boolean
  /** An incomplete trailing record was excluded from this live snapshot. */
  incomplete_tail: boolean
}

/** Hash-and-length summary for one elided canonical byte field. */
export interface BytePayloadSummary {
  /** BLAKE3 of the exact bytes. */
  blake3: string
  /** Exact byte count. */
  byte_count: bigint
  /** Event field name (`data`, `query`, or `response`). */
  field: string
}

/** Actor-consistent position in the canonical event stream. */
export interface CanonicalBoundary {
  /** Hash of the preceding event, or the event header at sequence zero. */
  event_hash: string
  /** Sequence number that will be assigned to the next event. */
  next_event_seq: bigint
  /** PTY output bytes included through this boundary. */
  output_bytes: bigint
}

/** Result of one bounded capability probe. */
export interface Capability {
  /** Stable diagnostic code. */
  code: string
  /** Human-readable probe result. */
  detail: string
  /** Stable capability identifier. */
  id: string
  /** Focused remediation when blocked. */
  remediation?: string | null
  /** Readiness result. */
  status: CapabilityStatus
}

/** Complete local capability report. */
export interface CapabilityReport {
  /** Source/build revision. */
  build_revision: string
  /** Ordered capability probes. */
  capabilities: Array<Capability>
  /** Pinned semantic engine revision. */
  ghostty_vt_revision: string
  /** Target architecture. */
  target_arch: string
  /** Target operating system. */
  target_os: string
  /** Package version. */
  termtrace_version: string
}

/** Stable capability readiness. */
export type CapabilityStatus = "ready" | "blocked"

/** Placement of the per-clip caption preset. */
export type CaptionLayout = "footer" | "overlay" | "none"

/** Completed native capture. */
export interface CaptureOutcome {
  /** Atomically committed visual directory. */
  directory: ExactPath
  /** Final native screenshot path. */
  screenshot: ExactPath
  /** Video path. */
  video?: ExactPath | null
}

/** How settled the observed state is. */
export type CaptureQuality = "live" | "stable" | "completed" | "incomplete"

/** Pixel viewport intended for native visual capture. */
export interface CaptureTarget {
  /** Viewport height in pixels. */
  height: number
  /** Balanced Ghostty window padding that fits a grid smaller than Ghostty's minimum window. */
  padding?: WindowPadding
  /** Viewport width in pixels. */
  width: number
}

/** One changed viewport cell. */
export interface CellChange {
  /** Complete later cell, absent outside later geometry. */
  after?: CellState | null
  /** Complete earlier cell, absent outside earlier geometry. */
  before?: CellState | null
  /** Zero-based column. */
  column: number
}

/** Terminal grid in cells. */
export interface CellGrid {
  /** Columns. */
  cols: number
  /** Rows. */
  rows: number
}

/** One visible cell in a semantic snapshot. */
export interface CellState {
  /** Zero-based column. */
  column: number
  /** Full UTF-8 grapheme cluster; empty for a blank cell. */
  grapheme: string
  /** Resolved explicit background, before terminal-default fallback. */
  resolved_background?: Rgb | null
  /** Resolved explicit foreground, before terminal-default fallback. */
  resolved_foreground?: Rgb | null
  /** Whether the cell is selected. */
  selected: boolean
  /** Visual style. */
  style: CellStyle
  /** Native width role; never inferred from the grapheme. */
  wide_kind: WideKind
}

/** Complete style attached to one terminal cell. */
export interface CellStyle {
  /** Background color source. */
  background: ColorSpec
  /** Blink attribute. */
  blink: boolean
  /** Bold attribute. */
  bold: boolean
  /** Faint attribute. */
  faint: boolean
  /** Foreground color source. */
  foreground: ColorSpec
  /** Inverse-video attribute. */
  inverse: boolean
  /** Invisible attribute. */
  invisible: boolean
  /** Italic attribute. */
  italic: boolean
  /** Overline attribute. */
  overline: boolean
  /** Strikethrough attribute. */
  strikethrough: boolean
  /** Upstream SGR underline value (`0` through `5`). */
  underline: number
  /** Underline color source. */
  underline_color: ColorSpec
}

/** Visible change-overlay mode. */
export type ChangeMode = "outline" | "dirty_rows" | "bounding_box" | "heatmap" | "trail" | "side_diff"

/** One nominal semantic-change region in source pixels. */
export interface ChangeRegion {
  /** Region height. */
  height: number
  /** Region width. */
  width: number
  /** Left source pixel. */
  x: number
  /** Top source pixel. */
  y: number
}

/** Truth source used by a change overlay. */
export type ChangeSource = "semantic_cells" | "encoded_pixels"

/** Explicit source and shape for per-frame change overlays. */
export interface ChangeVisualization {
  /** Number of output frames to retain a fading/held overlay. */
  hold_frames?: number
  /** Visible overlay form. */
  mode: ChangeMode
  /** Pixel-difference threshold for encoded input, ignored for semantic input. */
  pixel_threshold?: number | null
  /** Semantic scheduling or actual decoded adjacent pixels. */
  source: ChangeSource
}

/** One MIME representation of a clipboard write. */
export interface ClipboardRepresentation {
  /** Digest of the decoded representation data. */
  data: PayloadDigest
  /** MIME type of the representation. */
  mime: string
}

/** Clipboard a program asked to write. */
export type ClipboardTarget = "standard" | "selection" | "primary" | "other"

/** A system monotonic clock bracket around a session-relative clock sample. */
export interface ClockSample {
  /** Session-relative time sampled inside the system clock bracket. */
  session_monotonic_ns: bigint
  /** Midpoint of the system `CLOCK_MONOTONIC` bracket. */
  system_monotonic_ns: bigint
  /** Maximum error of the midpoint mapping, in nanoseconds. */
  uncertainty_ns: bigint
}

/** Deterministic common color-variable policy applied after caller overrides. */
export type ColorPolicy = "auto" | "always" | "never"

/** Cell color source before default-color fallback. */
export type ColorSpec = ColorSpecNone | ColorSpecPalette | ColorSpecRgb

/** No explicit color. */
export interface ColorSpecNone {
  kind: "none"
}

/** Index into the active 256-color palette. */
export interface ColorSpecPalette {
  kind: "palette"
  value: number
}

/** Explicit true color. */
export interface ColorSpecRgb {
  kind: "rgb"
  value: Rgb
}

/** Active terminal colors. */
export interface ColorState {
  /** Default/current background. */
  background: Rgb
  /** Explicit cursor color. */
  cursor?: Rgb | null
  /** Default/current foreground. */
  foreground: Rgb
  /** Active 256-color palette. */
  palette: Array<Rgb>
}

/** Canonical output byte that carried an OSC 133 mark. */
export interface CommandMark {
  /** Offset of that byte within the event. */
  event_byte_offset: bigint
  /** Canonical output event holding the mark's completing byte. */
  event_seq: bigint
}

/** Outcome of one control command step. */
export type CommandOutcome = ControlOutcome | PointerOutcome

/** Condition checked after input, in host millisecond units. */
export type ConditionSpec = ConditionSpecNone | ConditionSpecIdle | ConditionSpecScreen

/** Require a reaction, then an unchanged presented state. */
export interface ConditionSpecIdle {
  condition: "idle"
  /** Reaction preceding the interval. */
  reaction?: ReactionSpec | null
  /** Required unchanged interval. */
  stable_for_ms?: bigint | null
  /** Maximum wait. */
  timeout_ms?: bigint | null
}

/** Return once input is applied. */
export interface ConditionSpecNone {
  condition: "none"
}

/** Wait for one presented-screen predicate. */
export interface ConditionSpecScreen {
  condition: "screen"
  /** Presented-screen predicate. */
  predicate: ScreenPredicate
  /** Maximum wait. */
  timeout_ms?: bigint | null
}

/** The actor applied a control mutation that produces no further data. */
export interface ControlAcknowledgement {
  /** Canonical positions after the mutation. */
  boundaries: ActorBoundaries
  /** Identity of the request. */
  request_id: string
}

/** Structured actor context attached to a rejected or incomplete request. */
export interface ControlErrorDetails {
  /** Durable and observed source positions at the failure boundary. */
  boundaries: ActorBoundaries
  /** Latest presented-state hash at the actor ordering point. */
  current_presented_state_hash: string
  /** Exact attempted prefix when an admitted input sequence stopped early. */
  input_sequence?: InputSequenceFailureDetails | null
  /** Exact output or mutation scope covered by these details. */
  scope: string
  /** Bounded rich-projection validation or resource-limit facts. */
  screen_frame?: ScreenFrameFailureDetails | null
}

/** Acknowledged or marked control command outcome. */
export type ControlOutcome = ControlOutcomeAcknowledged | ControlOutcomeMarked

/** The actor applied the command. */
export interface ControlOutcomeAcknowledged {
  kind: "acknowledged"
}

/** The actor recorded a marker. */
export interface ControlOutcomeMarked {
  kind: "marked"
}

/** One semantic cursor region masked on a mapped presentation frame. */
export interface CursorMaskFrame {
  /** Honest semantic-to-native correlation level. */
  evidence_level: string
  /** Presentation frame receiving the mask. */
  output_frame: bigint
  /** Nominal cursor pixel region. */
  region: ChangeRegion
  /** Native source frame selected by presentation mapping. */
  source_frame: bigint
  /** Complete source semantic state hash. */
  state_hash: string
  /** Latest presentation-eligible semantic state used for cursor geometry. */
  state_id: bigint
}

/** Cursor state at one semantic boundary. */
export interface CursorState {
  /** Blink mode. */
  blinking: boolean
  /** Viewport column when available. */
  column?: number | null
  /** Whether the cursor lies in the viewport. */
  in_viewport: boolean
  /** Viewport row when available. */
  row?: number | null
  /** Visual shape. */
  style: CursorStyle
  /** Visibility mode. */
  visible: boolean
  /** Whether the cursor is on a wide-character tail. */
  wide_tail?: boolean | null
}

/** Cursor visual style. */
export type CursorStyle = "bar" | "block" | "underline" | "hollow_block"

export type DecimalU64 = bigint

/** Deterministic report from conservative session-wide detectors. */
export interface DiagnoseReport {
  /** Minimum changed-cell fraction used for broad redraw findings. */
  broad_change_fraction: string
  /** Ordered detector selection. */
  checks: Array<DiagnosticCheck>
  /** Deterministic findings. */
  findings: Array<DiagnosticFinding>
  /** Stable report format. */
  format: string
  /** Findings that were detected but not listed because their code reached the listing bound. */
  omitted_findings: Array<OmittedFindings>
  /** Page coordinates when CLI output intentionally contains a bounded subset. */
  output_page?: OutputPage | null
  /** Canonical session identity. */
  session_id: string
  /** Canonical event count anchoring the report. */
  source_event_count: bigint
  /** Final canonical event hash anchoring the report. */
  source_final_event_hash: string
  /** Report schema version. */
  version: number
}

/** Stable detector selectable by `diagnose --check`. */
export type DiagnosticCheck = "full_redraw" | "transient" | "dirty" | "cursor" | "modes" | "palette" | "query_loop" | "effects"

/** One conservative session-wide diagnostic finding. */
export interface DiagnosticFinding {
  /** Stable machine-readable finding code. */
  code: string
  /** Conservative confidence label. */
  confidence: FindingConfidence
  /** Exact neutral facts and limitations. */
  facts: Array<string>
  /** Native relationships, if independently available. */
  native_correlations: Array<NativeCorrelation>
  /** Supporting semantic state IDs. */
  state_ids: Array<bigint>
  /** Supporting VT token IDs. */
  token_ids: Array<bigint>
}

/** Stable identity for one semantic diff endpoint. */
export interface DiffEndpoint {
  /** Responsible event sequence. */
  event_seq?: bigint | null
  /** Ghostty dirty classification at this endpoint. */
  ghostty_dirty: DirtyState
  /** Rows Ghostty marked dirty at this endpoint. */
  ghostty_dirty_rows: Array<number>
  /** Session-monotonic timestamp. */
  monotonic_ns: bigint
  /** Requested endpoint selector. */
  point: StatePoint
  /** Whether the state was presentation eligible. */
  presented: boolean
  /** Complete semantic state hash. */
  state_hash: string
  /** Stream position of the state. */
  state_id: bigint
}

/** Global render dirtiness reported by Ghostty. */
export type DirtyState = "clean" | "partial" | "full"

/** Output persistence behavior selected for one canonical recording. */
export type DurabilityMode = "strict" | "bounded" | "throughput"

/** Chain-bound configuration for canonical output persistence. */
export interface DurabilityPolicy {
  /** Maximum uncommitted interval for bounded mode. */
  maximum_interval_ms?: bigint | null
  /** Maximum uncommitted PTY bytes for bounded mode. */
  maximum_uncommitted_bytes?: bigint | null
  /** Selected persistence behavior. */
  mode: DurabilityMode
}

/** Recorder-observed durability outcomes bound into completion evidence. */
export interface DurabilityRecord {
  /** Whether the final event boundary completed file synchronization. */
  final_sync_completed: boolean
  /** Largest PTY byte count pending synchronization. */
  maximum_observed_uncommitted_bytes: bigint
  /** Largest measured interval with output pending synchronization. */
  maximum_observed_uncommitted_ns: bigint
  /** Chain-bound configured policy. */
  policy: DurabilityPolicy
  /** Number of successful event-log data synchronizations after header creation. */
  sync_count: bigint
}

/** One source interval with rational speed and a generated final hold. */
export interface EditClip {
  /** Optional human presentation caption. */
  caption?: string | null
  /** Inclusive source selector. */
  from: EditSelector
  /** Generated hold duration after the final retimed source frame. */
  hold_ms?: bigint
  /** Playback speed as an exact rational. */
  speed?: RationalSpeed
  /** Exclusive source selector. */
  to: EditSelector
}

/** Typed edit-plan failure with the location it was found at. */
export interface EditFailure {
  /** Caller clip index, when the failure belongs to one clip. */
  clip_index?: bigint | null
  /** Stable category. */
  code: EditFailureCode
  /** Path of the rejected value inside the edit, such as `clips[1].from`. */
  field: string
  /** Marker label, when the failure concerns a marker. */
  marker?: string | null
  /** Human-readable reason. */
  reason: string
}

/** Stable category of an edit-plan failure, shared by every SDK. */
export type EditFailureCode = "invalid_edit_plan" | "invalid_selector" | "marker_not_found" | "marker_not_unique" | "selector_out_of_range" | "invalid_speed" | "invalid_caption" | "overlapping_clips" | "clip_out_of_range" | "invalid_annotation" | "invalid_subtitle" | "relative_resource_without_base" | "invalid_transform" | "invalid_change_visualization"

/** One immutable presentation edit plan. */
export interface EditPlan {
  /** Arbitrary normalized text annotation tracks. */
  annotations?: Array<TextAnnotation>
  /** Optional per-frame change overlay policy. */
  change_visualization?: ChangeVisualization | null
  /** Caller-ordered source clips. */
  clips: Array<EditClip>
  /** Delivery encoding contract. */
  delivery?: PresentationDelivery
  /** Presentation canvas and caption policy. */
  layout?: PresentationLayout
  /** User-facing video facade settings retained in the final manifest. */
  render_config?: VideoRenderConfig | null
  /** Caller-authored ASS subtitle resources. */
  subtitles?: Array<SubtitleTrack>
  /** Explicit visibly labeled pixel transforms. */
  transforms?: Array<PixelTransform>
  /** Edit-plan format version. */
  version: number
}

/** Unique marker or exact canonical monotonic timestamp. */
export interface EditSelector {
  /** Latest semantic state at or before this canonical event. */
  eventSeq?: bigint | null
  /** Unique canonical marker label. */
  marker?: string | null
  /** Exact canonical session offset in nanoseconds. */
  monotonicNs?: bigint | null
  /** Exact native source frame. */
  nativeFrame?: bigint | null
  /** Stream position of the state: output bytes and applied resizes before it. */
  stateId?: bigint | null
}

/** Where an edit plan comes from. */
export type EditSource = EditSourceFile | EditSourcePlan

/** JSON edit file; relative subtitle paths resolve beside it. */
export interface EditSourceFile {
  kind: "file"
  /** Edit-plan JSON file. */
  path: ExactPath
}

/** In-memory edit; relative subtitle paths need `resource_base`. */
export interface EditSourcePlan {
  /** Edit plan. */
  edit: EditPlan
  kind: "plan"
  /** Directory that relative subtitle paths are joined to. */
  resource_base?: ExactPath | null
}

/** Source environment used before explicit child overrides are applied. */
export type EnvironmentBase = "inherit" | "clear"

/** Caller-owned child-environment configuration. */
export interface EnvironmentSpec {
  /** Whether resolution begins with the recorder's ambient environment. */
  base?: EnvironmentBase
  /** Deterministic color-variable behavior. */
  color?: ColorPolicy
  /** Names copied from the recorder environment without placing values in arguments. */
  copy?: Array<string>
  /** Exact UTF-8-named values supplied by the caller. */
  set?: Record<string, Base64Bytes>
  /** Names removed from an inherited environment. */
  unset?: Array<string>
}

/** Classified failure of one step that did not abort its operation. */
export interface ErrorReport {
  /** Stable failure code. */
  code: string
  /** Whether the failure applied a mutation. */
  effect: MutationEffect
  /** Stable failure kind. */
  kind: string
  /** Human-readable description. */
  message: string
  /** Whether retrying can succeed. */
  retry: Retry
}

/** One ordered session event. */
export type Event = EventOutput | EventInputIntent | EventInputResult | EventResizeIntent | EventResizeResult | EventSignalIntent | EventSignalResult | EventMarker | EventApplication | EventQueryResponse | EventLifecycle | EventExit | EventHostError

/** Structured application instrumentation received over the control socket. */
export interface EventApplication {
  /** Caller-reported diagnostic payload. */
  data: ApplicationEvent
  /** Caller-owned request identity for durable deduplication. */
  operation_id: string
  type: "application"
}

/** Child process termination. */
export interface EventExit {
  /** Exit status when the process exited normally. */
  code?: number | null
  /** Signal number when signal termination occurred. */
  signal?: number | null
  /** Platform signal description when available. */
  signal_name?: string | null
  type: "exit"
}

/** Actor-owned durable follow position or independently validated completion. */
export type EventFollowPosition = EventFollowPositionLive | EventFollowPositionCompleted

/** Validated terminal completion, including its source-bound final event hash. */
export interface EventFollowPositionCompleted {
  /** Full completion; this is not a live-prefix claim. */
  completion: SessionCompletion
  kind: "completed"
}

/** Position authenticated by the live actor, not imported file metadata. */
export interface EventFollowPositionLive {
  /** Writer descriptor device. */
  device: bigint
  /** Latest synchronized position. */
  durable: LiveEventCursor
  /** Exact canonical header hash. */
  header_hash: string
  /** Writer descriptor inode. */
  inode: bigint
  kind: "live"
  /** Requested position, when retained and durable. */
  start?: LiveEventCursor | null
}

/** A host failure retained in the canonical timeline. */
export interface EventHostError {
  /** Stable diagnostic code. */
  code: string
  /** Redacted human-readable message. */
  message: string
  type: "host_error"
}

/** Durable intent to send bytes to the child PTY. */
export interface EventInputIntent {
  /** Optional caller-supplied reason for applying the input. */
  caller_intent?: string | null
  /** Exact emitted bytes. */
  data: Base64Bytes
  /** Structured semantic source of the exact emitted bytes. */
  descriptor: InputDescriptor
  /** High-level input category. */
  input_kind: InputKind
  /** Identifier paired with the corresponding result. */
  operation_id: string
  /** Canonical symbolic input, such as `ctrl+u` or `paste`. */
  semantic_input?: string | null
  /** Input origin. */
  source: InputSource
  /** BLAKE3 of canonical terminal modes used to encode this atom, when mode-aware. */
  terminal_modes_hash?: string | null
  type: "input_intent"
}

/** Result of one input intent. */
export interface EventInputResult {
  /** Bytes written before success or failure. */
  bytes_written: bigint
  /** Stable diagnostic code on failure. */
  error_code?: string | null
  /** Identifier from the corresponding intent. */
  operation_id: string
  /** Whether the entire byte sequence was applied. */
  status: OperationStatus
  type: "input_result"
}

/** A process or host lifecycle transition. */
export interface EventLifecycle {
  /** Transition kind. */
  lifecycle_kind: LifecycleKind
  /** Child process ID when available. */
  pid?: number | null
  type: "lifecycle"
}

/** A named point in the session timeline. */
export interface EventMarker {
  /** User-provided marker label. */
  label: string
  /** Caller-owned operation identifier used for durable deduplication. */
  operation_id: string
  type: "marker"
}

/** Model-specific byte-elided event metadata; application attributes are deliberately excluded. */
export type EventMetadata = EventMetadataOutput | EventMetadataInputIntent | EventMetadataInputResult | EventMetadataResizeIntent | EventMetadataResizeResult | EventMetadataSignalIntent | EventMetadataSignalResult | EventMetadataMarker | EventMetadataApplication | EventMetadataQueryResponse | EventMetadataLifecycle | EventMetadataExit | EventMetadataHostError

/** Metadata for the canonical Application variant. */
export interface EventMetadataApplication {
  /** Application lifecycle variant. */
  kind: ApplicationEventKind
  /** Actor request identity. */
  operation_id: string
  /** Parent span identity, when applicable. */
  parent_span_id?: string | null
  /** Caller-reported process ID. */
  process_id: number
  /** Emitting process instance. */
  process_instance_id: string
  /** Span identity, when applicable. */
  span_id?: string | null
  type: "application"
}

/** Metadata for the canonical Exit variant. */
export interface EventMetadataExit {
  /** Exit status when the process exited normally. */
  code?: number | null
  /** Signal number when signal termination occurred. */
  signal?: number | null
  /** Platform signal description when available. */
  signal_name?: string | null
  type: "exit"
}

/** Metadata for the canonical `HostError` variant. */
export interface EventMetadataHostError {
  /** Stable diagnostic code. */
  code: string
  /** Redacted human-readable message. */
  message: string
  type: "host_error"
}

/** Metadata for the canonical `InputIntent` variant. */
export interface EventMetadataInputIntent {
  /** Optional caller-supplied reason for applying the input. */
  caller_intent?: string | null
  /** Structured semantic source of the exact emitted bytes. */
  descriptor: InputDescriptor
  /** High-level input category. */
  input_kind: InputKind
  /** Identifier paired with the corresponding result. */
  operation_id: string
  /** Canonical symbolic input, such as `ctrl+u` or `paste`. */
  semantic_input?: string | null
  /** Input origin. */
  source: InputSource
  /** BLAKE3 of canonical terminal modes used to encode this atom, when mode-aware. */
  terminal_modes_hash?: string | null
  type: "input_intent"
}

/** Metadata for the canonical `InputResult` variant. */
export interface EventMetadataInputResult {
  /** Bytes written before success or failure. */
  bytes_written: bigint
  /** Stable diagnostic code on failure. */
  error_code?: string | null
  /** Identifier from the corresponding intent. */
  operation_id: string
  /** Whether the entire byte sequence was applied. */
  status: OperationStatus
  type: "input_result"
}

/** Metadata for the canonical Lifecycle variant. */
export interface EventMetadataLifecycle {
  /** Transition kind. */
  lifecycle_kind: LifecycleKind
  /** Child process ID when available. */
  pid?: number | null
  type: "lifecycle"
}

/** Metadata for the canonical Marker variant. */
export interface EventMetadataMarker {
  /** User-provided marker label. */
  label: string
  /** Caller-owned operation identifier used for durable deduplication. */
  operation_id: string
  type: "marker"
}

/** Output payload is represented by the projection byte summary. */
export interface EventMetadataOutput {
  type: "output"
}

/** Metadata for the canonical `QueryResponse` variant. */
export interface EventMetadataQueryResponse {
  /** Component that generated the elided query response. */
  responder: string
  type: "query_response"
}

/** Metadata for the canonical `ResizeIntent` variant. */
export interface EventMetadataResizeIntent {
  /** Columns after the resize. */
  cols: number
  /** Identifier paired with the corresponding result. */
  operation_id: string
  /** Total pixel height reported to the child. */
  pixel_height: number
  /** Total pixel width reported to the child. */
  pixel_width: number
  /** Rows after the resize. */
  rows: number
  type: "resize_intent"
}

/** Metadata for the canonical `ResizeResult` variant. */
export interface EventMetadataResizeResult {
  /** Stable diagnostic code on failure. */
  error_code?: string | null
  /** Identifier from the corresponding intent. */
  operation_id: string
  /** Whether the new geometry was applied. */
  status: OperationStatus
  type: "resize_result"
}

/** Metadata for the canonical `SignalIntent` variant. */
export interface EventMetadataSignalIntent {
  /** Stable display name such as `SIGINT`. */
  name: string
  /** Platform signal number. */
  number: number
  /** Identifier paired with the corresponding result. */
  operation_id: string
  type: "signal_intent"
}

/** Metadata for the canonical `SignalResult` variant. */
export interface EventMetadataSignalResult {
  /** Stable diagnostic code on failure. */
  error_code?: string | null
  /** Identifier from the corresponding intent. */
  operation_id: string
  /** Whether the signal syscall succeeded. */
  status: OperationStatus
  type: "signal_result"
}

/** Bytes read from the child PTY. */
export interface EventOutput {
  /** Exact bytes, including control sequences and invalid UTF-8. */
  data: Base64Bytes
  type: "output"
}

/** A bounded page of lifecycle history. */
export interface EventPage {
  /** Events in sequence order. */
  events: Array<TmuxEventRecord>
  /** First sequence of the following page, if any. */
  next_sequence?: bigint | null
}

/** A checked chain endpoint, not permission to skip validation of an imported cursor. */
export interface EventPageCursor {
  /** Hash immediately before `next_event_seq`, or header hash at zero. */
  event_hash: string
  /** Next scanned event, including events excluded by the filter. */
  next_event_seq: bigint
  /** File and session identity. */
  source: PageSource
}

/** Stable JSONL projection of one canonical event record. */
export interface EventProjection {
  /** Hash-and-length summaries when byte fields were elided. */
  byte_payloads: Array<BytePayloadSummary>
  /** Exact typed event when byte payloads were retained. */
  event?: Event | null
  /** Stable event variant name. */
  event_type: string
  /** Canonical event hash. */
  hash: string
  /** Non-byte event fields when exact payloads were elided. */
  metadata?: EventMetadata | null
  /** Whether exact byte fields were elided. */
  metadata_only: boolean
  /** Session-monotonic event timestamp. */
  monotonic_ns: bigint
  /** Separately observed timestamp, when the recorder retained one. */
  observed_monotonic_ns?: bigint | null
  /** Hash of the preceding canonical record or header. */
  prev_hash: string
  /** Canonical event sequence. */
  seq: bigint
}

/** A terminal capability query and corresponding response. */
export interface EventQueryResponse {
  /** Query bytes emitted by the child. */
  query: Base64Bytes
  /** Component that generated the response. */
  responder: string
  /** Response bytes sent back to the child. */
  response: Base64Bytes
  /** Inclusive offset of the query's final byte in that output event. */
  source_byte_offset?: bigint | null
  /** Canonical output event containing the end of the query. */
  source_event_seq?: bigint | null
  type: "query_response"
}

/** Durable intent to change terminal geometry. */
export interface EventResizeIntent {
  /** Columns after the resize. */
  cols: number
  /** Identifier paired with the corresponding result. */
  operation_id: string
  /** Total pixel height reported to the child. */
  pixel_height: number
  /** Total pixel width reported to the child. */
  pixel_width: number
  /** Rows after the resize. */
  rows: number
  type: "resize_intent"
}

/** Result of one resize intent. */
export interface EventResizeResult {
  /** Stable diagnostic code on failure. */
  error_code?: string | null
  /** Identifier from the corresponding intent. */
  operation_id: string
  /** Whether the new geometry was applied. */
  status: OperationStatus
  type: "resize_result"
}

/** Durable intent to signal the child process group. */
export interface EventSignalIntent {
  /** Stable display name such as `SIGINT`. */
  name: string
  /** Platform signal number. */
  number: number
  /** Identifier paired with the corresponding result. */
  operation_id: string
  type: "signal_intent"
}

/** Result of one signal intent. */
export interface EventSignalResult {
  /** Stable diagnostic code on failure. */
  error_code?: string | null
  /** Identifier from the corresponding intent. */
  operation_id: string
  /** Whether the signal syscall succeeded. */
  status: OperationStatus
  type: "signal_result"
}

/** Evidence retention requested for an owned terminal test. */
export type EvidencePolicy = "on_failure" | "always" | "manual"

export type ExactPath = string | Uint8Array

/** A valid expectation that did not match. Only this type is suitable for conversion into a runner's negatable mismatch value. */
export interface ExpectationMismatchError {
  /** Bounded actual summary. */
  actual: string
  /** Stable expectation family. */
  assertion: string
  /** Bounded screen excerpt, never the complete protocol payload. */
  excerpt: string
  /** Bounded expected summary. */
  expected: string
  /** Exact frame identity when a frame was available. */
  frame?: AssertionFrameIdentity | null
  /** Whether any diagnostic field was truncated. */
  truncated: boolean
}

/** One deterministic evidence-backed diagnostic finding. */
export interface ExplainFinding {
  /** Stable machine-readable finding code. */
  code: string
  /** Conservative confidence label. */
  confidence: FindingConfidence
  /** Exact neutral facts, without pixel-level inference. */
  facts: Array<string>
  /** VT token IDs supporting the finding. */
  token_ids: Array<bigint>
}

/** Independent page coordinates for each ordered explain collection. */
export interface ExplainOutputPage {
  /** Deterministic-finding page. */
  findings: OutputPage
  /** Semantic-state page. */
  states: OutputPage
  /** Lossless VT-token page. */
  tokens: OutputPage
}

/** Causal evidence around one canonical state point. */
export interface ExplainReport {
  /** Inclusive event-sequence context radius. */
  context_events: bigint
  /** Conservative deterministic findings. */
  findings: Array<ExplainFinding>
  /** Page coordinates when CLI output intentionally contains bounded subsets. */
  output_page?: ExplainOutputPage | null
  /** Canonical session identity. */
  session_id: string
  /** Canonical event count anchoring this report's source. */
  source_event_count: bigint
  /** Final canonical event hash anchoring this report's source. */
  source_final_event_hash: string
  /** Semantic transitions in the selected event context. */
  states: Array<StateRecord>
  /** Requested target selector. */
  target: StatePoint
  /** Resolved target semantic state. */
  target_state: StateRecord
  /** Lossless tokens in the selected event context. */
  tokens: Array<VtToken>
}

/** Result of exactly-once test finalization. */
export interface FinalizationReport {
  /** Canonical evidence retained by Termtrace. */
  evidence_directory?: string | null
  /** Outcome supplied by the wrapper or advanced caller. */
  outcome: TestOutcome
  /** Whether evidence actually survives (policy, failure, or unowned canonical data). */
  retained: boolean
  /** Non-primary bounded cleanup failures. */
  secondary_failures: Array<SecondaryFailure>
  /** Artifact paths verified through public Termtrace verifiers. */
  verified_artifacts: Array<string>
}

/** Conservative confidence label for a derived finding. */
export type FindingConfidence = "high" | "medium"

/** Requested rich-screen representation. */
export type FrameDetail = "none" | "text" | "spans" | "cells"

/** One completed, unregistered state-root recording eligible for explicit deletion. */
export interface GarbageCollectionCandidate {
  /** Complete bytes below the session directory. */
  bytes: bigint
  /** Completion-file modification timestamp used for retention ordering. */
  completed_unix_ns: bigint
  /** Canonical managed session directory. */
  session: ExactPath
  /** Immutable session identity. */
  session_id: string
}

/** Safe state-root retention plan or applied deletion report. */
export interface GarbageCollectionReport {
  /** Exact eligible recordings, retained in the report even after deletion. */
  candidates: Array<GarbageCollectionCandidate>
  /** Whether eligible candidates were actually deleted. */
  changes_applied: boolean
  /** Completed bytes remaining if the reported candidates are deleted. */
  completed_bytes_after_plan?: bigint
  /** Bytes in validated completed managed sessions at scan time, including named evidence. */
  completed_bytes_before?: bigint
  /** Newest completed, unregistered recordings protected regardless of age. */
  keep_latest: bigint
  /** Optional target for total completed managed-session bytes. */
  max_completed_bytes?: bigint | null
  /** Minimum completed-recording age in whole days. */
  older_than_days: bigint
  /** Completed managed recordings protected by a registry alias. */
  protected_registered: bigint
  /** Bytes exceeding the target after planning because protected evidence is retained. */
  quota_excess_bytes?: bigint
  /** Bytes reclaimed from canonical managed session directories. */
  reclaimed_bytes: bigint
  /** Session directories removed when `changes_applied` is true. */
  removed_sessions: bigint
  /** Stale alias registry bindings removed during collection. */
  removed_stale_aliases?: bigint
  /** Detached supervisor log files removed with deleted sessions. */
  removed_supervisor_files: bigint
  /** Malformed or insecure registry records isolated from this scan. */
  skipped_corrupt_registry_entries: bigint
  /** Live, incomplete, malformed, or non-directory entries never considered. */
  skipped_non_completed: bigint
  /** Owner-only Termtrace state root considered. */
  state_root: ExactPath
}

/** One viewport and measured grid considered by the exact-grid solver. */
export interface GeometryProbeAttempt {
  /** Grid, cell, and text-area dimensions reported by Ghostty. */
  report: TerminalGeometryReport
  /** Whole native viewport supplied to Ghostty. */
  target: CaptureTarget
}

/** Bounded capture-time search proving the selected viewport fits the canonical grid. */
export interface GeometrySolveReport {
  /** Ordered probe decisions, including the independently measured final target. */
  attempts: Array<GeometryProbeAttempt>
  /** Required canonical columns. */
  required_cols: number
  /** Required canonical rows. */
  required_rows: number
  /** Exact-grid selected viewport. */
  selected: CaptureTarget
}

/** A GIF loop is either infinite or absent (play once). */
export type GifLoop = "infinite" | "once"

/** Structured semantic provenance for exact input bytes. */
export type InputDescriptor = InputDescriptorRaw | InputDescriptorText | InputDescriptorKey | InputDescriptorPaste | InputDescriptorFocus | InputDescriptorPointer | InputDescriptorActivate

/** Semantic visible-text resolution followed by a mode-aware click. */
export interface InputDescriptorActivate {
  /** Current grid columns. */
  cols: number
  /** Pinned encoder identity. */
  encoder: string
  /** Zero-based exclusive matched column. */
  end_column: number
  kind: "activate"
  /** Presented-state hash used for resolution and encoding. */
  presented_state_hash: string
  /** Zero-based matched row. */
  row: number
  /** Current grid rows. */
  rows: number
  /** Zero-based first matched column. */
  start_column: number
  /** Cell chosen for the click. */
  target_column: number
  /** BLAKE3 of canonical JSON for the terminal mode snapshot. */
  terminal_modes_hash: string
  /** Exact matched text. */
  text: string
}

/** Focus transition encoded while DEC focus reporting was enabled. */
export interface InputDescriptorFocus {
  /** Pinned encoder identity. */
  encoder: string
  /** Whether the terminal gained (`true`) or lost (`false`) focus. */
  focused: boolean
  kind: "focus"
  /** BLAKE3 of canonical JSON for the terminal mode snapshot. */
  terminal_modes_hash: string
}

/** One semantic key encoded against a live terminal-mode snapshot. */
export interface InputDescriptorKey {
  /** Alt modifier. */
  alt: boolean
  /** Control modifier. */
  control: boolean
  /** Stable encoder identity. */
  encoder: string
  /** Stable symbolic key spelling. */
  key: string
  kind: "key"
  /** Shift modifier. */
  shift: boolean
  /** Super modifier. */
  super_key: boolean
  /** BLAKE3 of canonical JSON for the terminal mode snapshot. */
  terminal_modes_hash: string
}

/** Paste content encoded against live bracketed-paste mode. */
export interface InputDescriptorPaste {
  /** Whether the actor applied bracketed-paste wrappers. */
  bracketed: boolean
  kind: "paste"
  /** BLAKE3 of canonical JSON for the terminal mode snapshot. */
  terminal_modes_hash: string
}

/** Pointer operation encoded against live mouse modes and geometry. */
export interface InputDescriptorPointer {
  /** Button name for clicks, otherwise wheel direction. */
  button: string
  /** Complete click count, or zero for non-click operations. */
  click_count: number
  /** Terminal columns at encoding time. */
  cols: number
  /** Zero-based viewport column. */
  column: number
  /** Pinned encoder identity. */
  encoder: string
  /** Optional zero-based destination column for a drag. */
  end_column?: number | null
  /** Optional zero-based destination row for a drag. */
  end_row?: number | null
  kind: "pointer"
  /** Signed requested wheel lines, or zero for clicks. */
  lines: number
  /** `click`, `multi_click`, `drag`, or `scroll`. */
  operation: string
  /** Total terminal surface height in pixels. */
  pixel_height: number
  /** Total terminal surface width in pixels. */
  pixel_width: number
  /** Zero-based viewport row. */
  row: number
  /** Terminal rows at encoding time. */
  rows: number
  /** BLAKE3 of canonical JSON for the terminal mode snapshot. */
  terminal_modes_hash: string
}

/** Uninterpreted caller or host bytes. */
export interface InputDescriptorRaw {
  kind: "raw"
}

/** UTF-8 text intentionally entered as textual content. */
export interface InputDescriptorText {
  kind: "text"
}

/** User-facing interpretation of an input byte sequence. */
export type InputKind = "raw" | "text" | "keys" | "paste" | "focus" | "pointer"

/** Canonical result coordinates for one atom in a successful input sequence. */
export interface InputSequenceAtomOutcome {
  /** Exact number of PTY bytes written. */
  bytes_written: bigint
  /** Semantic category retained in canonical evidence. */
  input_kind: InputKind
  /** Canonical input-intent event sequence. */
  intent_event_seq: bigint
  /** Caller-owned operation identifier for this atom. */
  operation_id: string
  /** Canonical input-result event sequence. */
  result_event_seq: bigint
}

/** Recoverable accounting for an input sequence that stopped during PTY delivery. */
export interface InputSequenceFailureDetails {
  /** Total number of requested atoms. */
  atom_count: number
  /** Number of preceding atoms applied completely. */
  atoms_applied: number
  /** Total bytes written across all attempted atoms. */
  bytes_written: bigint
  /** Bytes written for the failed atom; nonzero means application may be partial. */
  failed_atom_bytes_written: bigint
  /** Zero-based atom whose write failed. */
  failed_atom_index: number
  /** Caller-owned operation identifier for the failed atom. */
  failed_operation_id: string
  /** First canonical input-intent event in the attempted prefix. */
  first_intent_event_seq: bigint
  /** Canonical failed input-result event. */
  last_result_event_seq: bigint
  /** First atom safe to submit independently with a new request ID. */
  resume_from_atom_index: number
  /** Caller-owned sequence request identifier. */
  sequence_id: string
}

/** Origin of bytes sent to the child process. */
export type InputSource = "human" | "agent" | "host"

/** Region validation failure reason. */
export type InvalidRegionReason = "cells_require_explicit_region" | "empty" | "reversed" | "out_of_bounds"

/** Both task-level reports derived in one canonical replay/diagnostic pass. */
export interface LastActionAnalysis {
  /** Range-restricted diagnostics. */
  diagnostics: LastActionDiagnosticsReport
  /** Compact action diff and warnings. */
  inspection: LastActionReport
}

/** Canonical request and intent/result boundaries defining one input action. */
export interface LastActionBoundary {
  /** Number of input atoms in the request. */
  atom_count: number
  /** Bytes confirmed written across every atom. */
  bytes_written: bigint
  /** First stable result error code, when any atom failed. */
  error_code?: string | null
  /** Intended input byte count without the input bytes themselves. */
  intended_bytes: bigint
  /** First canonical input-intent event sequence. */
  intent_event_seq: bigint
  /** Ordered canonical operation identifiers for every input atom. */
  operation_ids: Array<string>
  /** Exclusive canonical event boundary after request execution and postcondition evaluation. */
  request_event_end_seq: bigint
  /** Inclusive canonical event boundary before request execution. */
  request_event_start_seq: bigint
  /** Caller-owned request identifier when the action was durably journaled. */
  request_id?: string | null
  /** Last canonical input-result event sequence. */
  result_event_seq: bigint
  /** Aggregate applied or failed result. */
  status: OperationStatus
}

/** Diagnostics restricted to the semantic and token range after the last input action. */
export interface LastActionDiagnosticsReport {
  /** Canonical input action boundary. */
  action: LastActionBoundary
  /** Bounded findings supported inside the selected action range. */
  findings: Array<DiagnosticFinding>
  /** Whether additional findings were omitted. */
  findings_truncated: boolean
  /** Stable report format. */
  format: string
  /** Earlier semantic state identifier. */
  from_state_id: bigint
  /** Canonical session identity. */
  session_id: string
  /** Final canonical source hash. */
  source_final_event_hash: string
  /** Later semantic state identifier. */
  to_state_id: bigint
  /** Report schema version. */
  version: number
}

/** Compact semantic result of the most recent canonical input action. */
export interface LastActionReport {
  /** Canonical input action boundary. */
  action: LastActionBoundary
  /** Total changed semantic cells. */
  cells_changed: bigint
  /** Complete cursor movement when the cursor changed. */
  cursor_movement?: ValueChange | null
  /** Stable report format. */
  format: string
  /** Earlier semantic state identifier. */
  from_state_id: bigint
  /** Sorted terminal mode changes. */
  mode_changes: Array<ActionModeChange>
  /** Child output generated after the canonical input intent. */
  output_generated: ActionOutputSummary
  /** Whether the resulting semantic screen was presentation eligible. */
  presentation_eligible: boolean
  /** Number of presentation-eligible semantic boundaries after the action. */
  presentation_eligible_states: bigint
  /** Sorted zero-based viewport rows with semantic cell changes. */
  rows_changed: Array<number>
  /** Canonical session identity. */
  session_id: string
  /** Shell command boundaries after the action, when the shell sent OSC 133 marks. */
  shell_command?: ActionShellCommand | null
  /** Final canonical source hash. */
  source_final_event_hash: string
  /** Later semantic state identifier. */
  to_state_id: bigint
  /** Report schema version. */
  version: number
  /** Bounded conservative warnings. */
  warnings: Array<ActionWarning>
  /** Whether additional warnings were omitted. */
  warnings_truncated: boolean
}

/** Launch configuration in host form. */
export interface LaunchOptions {
  /** Initial terminal columns. */
  cols?: number | null
  /** Child working directory; the caller's when absent. */
  cwd?: string | null
  /** Complete child-environment policy; inherit everything when absent. */
  environment?: EnvironmentSpec | null
  /** Terminate after this long without input, output, or control activity. */
  idle_timeout_ms?: bigint | null
  /** Terminate after this wall-clock duration. */
  max_duration_ms?: bigint | null
  /** Terminate after this many output bytes, retaining all of them. */
  output_byte_budget?: bigint | null
  /** Initial terminal rows. */
  rows?: number | null
  /** Recorder readiness deadline. */
  startup_timeout_ms?: bigint | null
  /** Visual profile committed into the session; legacy ambient defaults when absent. */
  visual_profile?: VisualProfile | null
}

/** Why a process lifecycle transition occurred. */
export type LifecycleKind = "spawned" | "output_closed" | "stopping"

/** An actor-owned position immediately after a canonical record (or the header). */
export interface LiveEventCursor {
  /** Hash of the preceding record or header. */
  event_hash: string
  /** Byte offset of the next record. */
  file_offset: bigint
  /** Timestamp of the preceding record, or zero at the header. */
  monotonic_ns: bigint
  /** Sequence of the next record. */
  next_event_seq: bigint
}

/** Match-count condition a locator wait requires. */
export type LocatorCondition = "exists" | "absent" | LocatorConditionCount

/** Exactly this many matches. */
export interface LocatorConditionCount {
  count: bigint
}

/** Typed details of a locator query, timeout, or cancellation failure. */
export type LocatorFailure = QueryError | LocatorWaitFailure

/** A locator wait that timed out or was cancelled, with the identity of its last frame. */
export type LocatorWaitFailure = LocatorWaitFailureLocatorTimeout | LocatorWaitFailureCancelled

/** The wait was cancelled. */
export interface LocatorWaitFailureCancelled {
  /** Projection identity of the last frame, when one was captured. */
  frame_hash?: string | null
  kind: "cancelled"
  /** Bounded normalized query. */
  query: QueryDescription
  /** Semantic state identity of the last frame, when one was captured. */
  state_hash?: string | null
}

/** The wait timed out. */
export interface LocatorWaitFailureLocatorTimeout {
  /** Projection identity of the last frame, when one was captured. */
  frame_hash?: string | null
  kind: "locator_timeout"
  /** Bounded normalized query. */
  query: QueryDescription
  /** Semantic state identity of the last frame, when one was captured. */
  state_hash?: string | null
}

/** Discriminator for actual non-actor management progress. */
export type ManagementFailure = ManagementFailureRegistryRestart | ManagementFailureTmux | ManagementFailureOriginalCapture

/** Original-execution capture lifecycle facts; the application may have run. */
export interface ManagementFailureOriginalCapture {
  facts: OriginalCaptureFacts
  operation: "original_capture"
}

/** Registry restart workflow facts. */
export interface ManagementFailureRegistryRestart {
  facts: RestartFailureFacts
  operation: "registry_restart"
}

/** Tmux management workflow facts. */
export interface ManagementFailureTmux {
  facts: TmuxFailureFacts
  operation: "tmux"
}

/** Canonical position of an appended marker. */
export interface MarkOutcome {
  /** Canonical positions after the marker. */
  boundaries: ActorBoundaries
  /** Canonical event containing the marker. */
  event_seq: bigint
  /** Normalized marker label. */
  label: string
  /** Identity of the request. */
  request_id: string
}

/** Canonical marker with duplicate and nominal-frame context. */
export interface MarkerRecord {
  /** Canonical event sequence. */
  event_seq: bigint
  /** Marker label. */
  label: string
  /** Canonical session offset. */
  monotonic_ns: bigint
  /** Honest native correlation level for the nominal frame. */
  native_evidence_level: string
  /** Nominal source frame at the requested rate. */
  nominal_frame: bigint
  /** Total occurrences of this label. */
  occurrence_count: bigint
  /** One-based position of this marker among occurrences of the same label. */
  occurrence_index: bigint
  /** Whether an associated presentation-eligible semantic state exists. */
  presentation_eligible: boolean
  /** Complete hash of the associated semantic state. */
  semantic_state_hash?: string | null
  /** Latest presentation-eligible semantic state at or before the marker. */
  semantic_state_id?: bigint | null
}

/** Options for the separate version-one cells comparison format. Masks exclude unstable comparison cells. They do not redact canonical evidence. */
export interface MaskedSnapshotOptions {
  /** Compare cursor facts too. */
  include_cursor?: boolean
  /** Compare terminal modes too. */
  include_modes?: boolean
  /** Nonempty, absolute, half-open rectangles inside the returned region. */
  mask_regions?: Array<ScreenRegion>
  /** Only `cells` is supported. Omission selects cells. */
  profile?: SnapshotProfile
}

/** One absolute half-open range covered by a match. */
export interface MatchRange {
  /** Exclusive absolute terminal column. */
  end_column: number
  /** Absolute row. */
  row: number
  /** Inclusive absolute terminal column. */
  start_column: number
}

/** Bounded cell evidence retained by a match. */
export interface MatchedCell {
  /** Complete source and resolved appearance. */
  appearance: ScreenAppearance
  /** Absolute terminal column of the authoritative head cell. */
  column: number
  /** Complete grapheme cluster. */
  grapheme: string
  /** Absolute row. */
  row: number
  /** Authoritative native width role. */
  wide_kind: WideKind
  /** Terminal columns covered: 2 for a wide head, otherwise 1. */
  width: number
}

/** Versioned SDK envelope. Data only; does not promise runtime capture/export support. */
export interface MediaArtifactResult {
  /** Exact OS directory bytes, standard padded Base64 in JSON. */
  directory_bytes: Base64Bytes
  /** Manifest is authoritative for mappings and publication facts. */
  manifest: ArtifactManifestRef
  /** Complete member metadata. */
  members: Array<SdkArtifactMember>
  /** Product classification. */
  product_kind: ArtifactProductKind
  /** Product truth. */
  provenance: ArtifactProvenance
  /** Canonical source boundary. */
  source: ArtifactSourceRef
  /** Exactly 1. Decoders must reject other versions. */
  version: number
}

/** Terminal modes relevant to presentation and input. */
export interface ModeState {
  /** Any alternate-screen mode. */
  alternate_screen: boolean
  /** DEC bracketed-paste mode 2004. */
  bracketed_paste: boolean
  /** DEC synchronized-output mode 2026. */
  synchronized_output: boolean
  /** Complete stable inventory of known ANSI and DEC modes. */
  values: Array<TerminalMode>
  /** DEC auto-wrap mode 7. */
  wraparound: boolean
}

/** Stable mouse button identity accepted by the pinned encoder. */
export type MouseButton = "left" | "middle" | "right" | "wheel_up" | "wheel_down"

/** Effect a request had on the child, its PTY, or canonical evidence. The recording actor states this fact in every rejection. Clients only synthesize it for transport outcomes the actor could not report. */
export type MutationEffect = MutationEffectNotApplied | MutationEffectPartial | MutationEffectApplied | MutationEffectUnknown

/** Every requested mutation was applied. */
export interface MutationEffectApplied {
  kind: "applied"
}

/** Nothing was applied. */
export interface MutationEffectNotApplied {
  kind: "not_applied"
}

/** A known prefix of input atoms was applied completely, and nothing after it. */
export interface MutationEffectPartial {
  /** Number of leading atoms applied completely. */
  completed_atoms: number
  kind: "partial"
}

/** The request may or may not have been applied. */
export interface MutationEffectUnknown {
  kind: "unknown"
}

/** Offline status of the disposable mutation request journal. */
export type MutationJournalStatus = "missing" | "verified" | "invalid"

/** One source-bound relationship to a native artifact. */
export interface NativeCorrelation {
  /** Nominal encoded frame when clock mapping supplied one. */
  frame?: bigint | null
  /** Honest evidence level. */
  level: NativeEvidenceLevel
  /** Exact limitation on the relationship. */
  limitation: string
  /** Relative native manifest path. */
  manifest: ExactPath
  /** Related semantic state. */
  state_id: bigint
}

/** Strength of evidence relating a semantic finding to native presentation. */
export type NativeEvidenceLevel = "presentation_eligible" | "scheduled_for_frame" | "post_endpoint_stable_capture" | "frame_observed"

/** Native facts observed while resolving a native profile. */
export interface NativeProfileReport {
  /** Requested families Ghostty discovered under their exact names. */
  discovered_font_families: Array<string>
  /** Display DPI used for the geometry probe and every later native launch. */
  dpi: number
  /** BLAKE3 of the exact effective configuration reported by Ghostty. */
  effective_config_hash: string
  /** Exact-grid viewport search at that DPI. */
  geometry: GeometrySolveReport
  /** BLAKE3 of the Ghostty executable that resolved the profile. */
  ghostty_executable_blake3: string
  /** Installed Ghostty version report. */
  ghostty_version: string
  /** BLAKE3 of the exact replay-profile input bytes. */
  replay_profile_hash: string
  /** Font families named by the effective configuration. */
  requested_font_families: Array<string>
}

/** Native Ghostty settings. Absent fields keep the base configuration's value. */
export interface NativeVisualProfile {
  /** Default background as `#rrggbb`. */
  background?: string | null
  /** Absolute Ghostty config file replacing the isolated Ghostty-default base. */
  config?: string | null
  /** Display DPI used by every native probe, launch, and replay; 96 when absent. */
  dpi?: number | null
  /** Exact installed font family; no substitute is accepted. */
  font_family?: string | null
  /** Font size in points as an exact decimal string, such as `"15"` or `"13.5"`. */
  font_size_points?: string | null
  /** Default foreground as `#rrggbb`. */
  foreground?: string | null
  /** Window padding in points on each side; zero is an explicit zero. */
  padding_points?: PaddingPoints | null
}

/** One annotation resolved to a half-open output-frame interval. */
export interface NormalizedAnnotation {
  /** Stable caller annotation index. */
  annotation_index: bigint
  /** Exclusive presentation frame. */
  output_end_frame: bigint
  /** Inclusive presentation frame. */
  output_start_frame: bigint
  /** Exact anchored placement. */
  position: AnnotationPosition
  /** Visible text/background style. */
  style: AnnotationStyle
  /** Per-frame dynamic text template. */
  template?: string | null
  /** Literal text, when the track is static. */
  text?: string | null
  /** Stable overlay ordering. */
  z_order: number
}

/** Semantic delta nominally scheduled onto presentation frames. */
export interface NormalizedChangeFrame {
  /** Nominal changed-cell pixel regions. */
  changed_regions: Array<ChangeRegion>
  /** Rows reported dirty by the semantic engine. */
  dirty_rows: Array<number>
  /** Honest correlation level for this overlay input. */
  evidence_level: string
  /** Output frames mapped to this source frame. */
  output_frames: Array<bigint>
  /** Nominal native frame selected by clock arithmetic. */
  source_frame: bigint
  /** Complete semantic state hash. */
  state_hash: string
  /** Semantic state that introduced the delta. */
  state_id: bigint
}

/** Resolved clip and exact source/output frame decisions. */
export interface NormalizedClip {
  /** Optional caption retained as presentation content. */
  caption?: string | null
  /** Generated hold frame count. */
  hold_frame_count: bigint
  /** Exclusive output frame after retiming and hold. */
  output_end_frame: bigint
  /** First output frame for this clip. */
  output_start_frame: bigint
  /** Retimed source-derived frame count. */
  retimed_frame_count: bigint
  /** Exclusive source video frame. */
  source_end_frame: bigint
  /** Canonical half-open source interval. */
  source_range: SourceRange
  /** Inclusive source video frame. */
  source_start_frame: bigint
  /** Exact playback speed. */
  speed: RationalSpeed
}

/** Edit plan with resolved resource paths and its path-independent identity. */
export interface NormalizedEdit {
  /** Validated edit; relative subtitle paths are joined to the resource base. */
  edit: EditPlan
  /** BLAKE3 of the edit with subtitle paths replaced by their content hashes. */
  edit_plan_hash: string
}

/** Validated subtitle input with stable content identity. */
export interface NormalizedSubtitle {
  /** BLAKE3 of the exact imported ASS bytes. */
  resource_hash: string
  /** Resolved caller resource path used for this export. */
  source_path: ExactPath
}

export type NullableString = string | null

/** Truth-labeled observation of one terminal state. */
export interface Observation {
  /** Source boundary observed or validated. */
  boundary: CanonicalBoundary
  /** Exact canonical session path bytes when evidence exists. */
  canonical_session?: Base64Bytes | null
  /** Settlement quality. */
  capture_quality: CaptureQuality
  /** Responsible canonical event sequence when known. */
  event_seq?: bigint | null
  /** Compact stable frame. */
  frame: ObservationFrame
  /** Integrity relationship to canonical execution evidence. */
  integrity: ObservationIntegrity
  /** Session-monotonic timestamp when known. */
  monotonic_ns?: bigint | null
  /** BLAKE3 of canonical JSON for `frame`. */
  payload_hash: string
  /** Whether this state was presentation eligible. */
  presented: boolean
  /** Service path used to resolve this state. */
  resolution: ObservationResolution
  /** Canonical session ID when evidence exists. */
  session_id?: string | null
  /** Whether canonical execution evidence was durably completed. */
  source_completed: boolean
  /** Source category. */
  source_kind: ObservationSourceKind
  /** BLAKE3 of the complete semantic state. */
  state_hash: string
  /** Semantic state ID when replay assigned one. */
  state_id?: bigint | null
  /** Requested state point. */
  state_point: StatePoint
  /** Rendering truth layer. */
  truth_layer: TruthLayer
}

/** Compact stable frame shared by CLI and clients. */
export interface ObservationFrame {
  /** Terminal columns. */
  cols: number
  /** Cursor column when it lies in the viewport. */
  cursor_column?: number | null
  /** Cursor row when it lies in the viewport. */
  cursor_row?: number | null
  /** Whether the cursor is visible. */
  cursor_visible: boolean
  /** Stable terminal-mode facts. */
  modes: ModeState
  /** Terminal rows. */
  rows: number
  /** Plain text for the complete viewport. */
  viewport_text: string
}

/** Integrity relationship between an observation and canonical evidence. */
export type ObservationIntegrity = "canonical_chained" | "unchained"

/** Service path used to resolve an observation without changing its meaning. */
export type ObservationResolution = "actor_snapshot" | "canonical_replay" | "validated_checkpoint" | "ansi_interpretation"

/** Source category for one observation. */
export type ObservationSourceKind = "live_session" | "completed_session" | "interrupted_session" | "ansi_stream" | "imported_ansi" | "one_off_command"

/** PIDs of live observers. */
export interface Observers {
  /** Supervisor process, if one holds its lock. */
  supervisor?: number | null
  /** Watchdog process, if one holds its lock. */
  watchdog?: number | null
}

/** Count of detected findings of one code that a report does not list. */
export interface OmittedFindings {
  /** Stable machine-readable finding code. */
  code: string
  /** Detected findings with this code beyond the listed ones. */
  count: bigint
}

/** Result of a state-changing operation. */
export type OperationStatus = "applied" | "failed"

/** What a caller learns when an original capture did not publish, or did not finish in time. The captured application may have run and completed; `completion` is its canonical outcome when one validates. This is a lifecycle failure, not an actor mutation, so it carries no request effect and no automatic retry. */
export interface OriginalCaptureFacts {
  /** Teardown, preservation or reaping failure reported beside the primary failure. */
  cleanup?: string | null
  /** Failure code; `None` when finalization did not reach a final state before the deadline. */
  code?: OriginalCaptureFailure | null
  /** Validated canonical completion, when one exists. */
  completion?: SessionCompletion | null
  /** Primary failure description. */
  reason: string
  /** Exact canonical session path bytes. */
  session: Base64Bytes
  /** Supervisor state observed when the error was built, including derived `abandoned`. */
  state: OriginalCaptureState
  /** Paths verified to exist when the error was built. */
  surviving: Array<SurvivingPathRecord>
}

/** Stable code for an original capture that did not publish an artifact. */
export type OriginalCaptureFailure = "cancelled" | "acquisition_failed" | "grid_mismatch" | "supervisor_abandoned" | "recording_failed" | "publication_failed" | "startup_failed"

/** Terminal failure facts written by the supervisor. */
export interface OriginalCaptureFailureRecord {
  /** Teardown or preservation failure reported beside the primary failure. */
  cleanup?: string | null
  /** Stable failure code. */
  code: OriginalCaptureFailure
  /** Primary failure description. */
  reason: string
  /** Paths that existed after bounded cleanup; nothing else is implied to exist. */
  surviving: Array<SurvivingPathRecord>
}

/** Supervisor lifecycle state. */
export type OriginalCaptureState = "acquiring" | "live" | "finalizing" | "published" | "failed" | "cancelled" | "abandoned"

/** Owner-only supervisor status, published beside the session as `<session>.original-capture.json`. Not canonical evidence. */
export interface OriginalCaptureStatus {
  /** Pixel source; `private_x11` is the only supervised backend. */
  backend: string
  /** Captured display size. */
  canvas: PixelCanvas
  /** Failure facts, present only in `failed` and `cancelled`. */
  failure?: OriginalCaptureFailureRecord | null
  /** Exactly [`ORIGINAL_CAPTURE_STATUS_FORMAT`]. */
  format: string
  /** Acquisition frame rate. */
  fps: number
  /** Requested exact grid, or the observed grid once live for a viewport-derived grid. */
  grid?: CellGrid | null
  /** Published artifact, present only in `published`. */
  result?: MediaArtifactResult | null
  /** Canonical session this supervisor records, written when the capture becomes live and kept afterwards; `None` only before the session exists. */
  session_id?: string | null
  /** Supervisor state as written; readers derive `abandoned`. */
  state: OriginalCaptureState
  /** Supervisor process ID. */
  supervisor_pid: DecimalU64
  /** Supervisor kernel start ticks, so a reused PID never reads as the supervisor. */
  supervisor_start_ticks: DecimalU64
  /** Exactly [`ORIGINAL_CAPTURE_STATUS_VERSION`]. */
  version: number
}

/** Original-capture launch configuration in host form. `grid` and `viewport` are exclusive, and the canonical `rows`/`cols` options do not exist here: the captured terminal owns its grid. */
export interface OriginalLaunchOptions {
  /** Registry alias bound once the session is live. */
  alias?: string | null
  /** Constant capture canvas; the terminal window when absent. */
  canvas?: PixelCanvas | null
  /** Child working directory; the caller's when absent. */
  cwd?: string | null
  /** Complete child-environment policy; inherit everything when absent. */
  environment?: EnvironmentSpec | null
  /** Acquisition rate: 24, 30 or 60. */
  fps?: number | null
  /** Ghostty configuration resolved into the recorded visual profile. */
  ghostty_config?: string | null
  /** Exact initial grid. */
  grid?: CellGrid | null
  /** Terminate after this long without input, output, or control activity. */
  idle_timeout_ms?: bigint | null
  /** Terminate after this wall-clock duration. */
  max_duration_ms?: bigint | null
  /** Terminate after this many output bytes, retaining all of them. */
  output_byte_budget?: bigint | null
  /** Acquisition and recorder startup deadline. */
  startup_timeout_ms?: bigint | null
  /** Registry state root for `alias`. */
  state_root?: string | null
  /** Initial Ghostty window size in pixels; the grid follows its font metrics. */
  viewport?: PixelCanvas | null
  /** Visual profile with a native section; replaces `ghostty_config`. */
  visual_profile?: VisualProfile | null
}

/** A finished original recording whose capture was published. */
export interface OriginalRecording {
  /** Whether the application exited 0 with no signal, resource limit or recorder failure. A published capture of a failing application is still a successful capture. */
  canonical_success: boolean
  /** Published `visual-original` artifact. */
  capture: MediaArtifactResult
  /** Validated canonical completion of the captured application. */
  completion: SessionCompletion
}

/** Source of one presentation output frame. */
export interface OutputFrameSource {
  /** Caller-ordered clip index. */
  clip_index: bigint
  /** `source` for retimed frames or `hold` for generated duplicates. */
  operation: string
  /** Source frame shown on the output frame. */
  source_frame: bigint
}

/** Deterministic page coordinates for one ordered collection. */
export interface OutputPage {
  /** Requested maximum, or none when every remaining item was requested. */
  limit?: bigint | null
  /** Offset for the next page when more items remain. */
  next_offset?: bigint | null
  /** Effective zero-based offset, clamped to the collection length. */
  offset: bigint
  /** Number of items included in this response. */
  returned: bigint
  /** Complete matching collection length when pagination reaches closure. */
  total?: bigint | null
  /** Whether more matching items remain after this page. */
  truncated: boolean
}

/** A byte position anchored to the chain endpoint enclosing that output prefix. */
export interface OutputPageCursor {
  /** Exact offset in the concatenated PTY output stream. */
  byte_offset: bigint
  /** Chain endpoint through the event containing the last returned byte. */
  events: EventPageCursor
}

/** Symmetric window padding in points per side. */
export interface PaddingPoints {
  /** Left and right padding. */
  x: number
  /** Top and bottom padding. */
  y: number
}

/** Identity of the canonical file opened for a page. Display paths are not authoritative. */
export interface PageSource {
  /** Device of the canonical root descriptor. */
  device: bigint
  /** Hash of the exact canonical header bytes. */
  header_hash: string
  /** Inode of the canonical root descriptor. */
  inode: bigint
  /** Immutable canonical session identity. */
  session_id: string
  /** Exact absolute canonical session directory bytes. */
  session_path: Base64Bytes
  /** Non-authoritative display only; never used to select or authenticate a source. */
  session_path_display: string
}

/** Length and short hash of a payload that may hold a secret, without the payload. */
export interface PayloadDigest {
  /** Leading hex characters of the payload's BLAKE3 hash. */
  blake3: string
  /** Payload length in bytes. */
  byte_length: bigint
}

/** Serializable uncertain-operation summary. */
export interface PendingOperationReport {
  /** Intended input byte count. */
  expected_bytes?: bigint | null
  /** Intent sequence number. */
  intent_seq: bigint
  /** Stable operation kind. */
  kind: string
  /** Operation identifier. */
  operation_id: string
}

/** Captured display size in pixels; constant for the whole video. */
export interface PixelCanvas {
  /** Height in pixels. */
  height: number
  /** Width in pixels. */
  width: number
}

/** Explicit presentation-only pixel transform. */
export type PixelTransform = PixelTransformCursorMask | PixelTransformCrop | PixelTransformZoom | PixelTransformResize | PixelTransformCallout

/** Draw a visibly labeled rectangular callout. */
export interface PixelTransformCallout {
  /** Callout height. */
  height: number
  /** Optional visible label. */
  label?: string | null
  transform: "callout"
  /** Callout width. */
  width: number
  /** Left callout pixel. */
  x: number
  /** Top callout pixel. */
  y: number
}

/** Crop to an exact source rectangle. */
export interface PixelTransformCrop {
  /** Rectangle height. */
  height: number
  transform: "crop"
  /** Rectangle width. */
  width: number
  /** Left source pixel. */
  x: number
  /** Top source pixel. */
  y: number
}

/** Replace cursor pixels during presentation composition. */
export interface PixelTransformCursorMask {
  transform: "cursor_mask"
}

/** Resize to an exact presentation canvas. */
export interface PixelTransformResize {
  /** Output height in pixels. */
  height: number
  transform: "resize"
  /** Output width in pixels. */
  width: number
}

/** Scale by one exact rational factor. */
export interface PixelTransformZoom {
  /** Scale denominator. */
  denominator: number
  /** Scale numerator. */
  numerator: number
  transform: "zoom"
}

/** One high-level cell-addressed pointer operation. */
export type PointerOperation = PointerOperationClick | PointerOperationMultiClick | PointerOperationDrag | PointerOperationScroll

/** Press and release one button at a cell. */
export interface PointerOperationClick {
  /** Button to click. */
  button: MouseButton
  kind: "click"
}

/** Press at the command cell, emit motion samples, and release at an end cell. */
export interface PointerOperationDrag {
  /** Button held throughout the drag. */
  button: MouseButton
  kind: "drag"
  /** Number of motion samples, including the destination. */
  steps: number
  /** Zero-based destination column. */
  to_column: number
  /** Zero-based destination row. */
  to_row: number
}

/** Repeat a complete press/release click a bounded number of times. */
export interface PointerOperationMultiClick {
  /** Button to click. */
  button: MouseButton
  /** Number of complete clicks. */
  count: number
  kind: "multi_click"
}

/** Emit signed vertical wheel ticks at a cell. */
export interface PointerOperationScroll {
  kind: "scroll"
  /** Negative scrolls up and positive scrolls down; zero is invalid. */
  lines: number
}

/** Typed result of one pointer, focus, or text-activation request. */
export type PointerOutcome = PointerOutcomeApplied | PointerOutcomeNoop | PointerOutcomeActivated

/** Visible text was resolved and its center clicked. */
export interface PointerOutcomeActivated {
  /** Canonical positions after application. */
  boundaries: ActorBoundaries
  /** Exact bytes the actor wrote. */
  bytes: Base64Bytes
  /** Number of bytes written completely. */
  bytes_written: bigint
  /** Zero-based exclusive end column. */
  end_column: number
  /** Canonical input-intent event. */
  intent_event_seq: bigint
  kind: "activated"
  /** Presented-state hash the match was resolved in. */
  presented_state_hash: string
  /** Identity of the request, also its canonical operation identity. */
  request_id: string
  /** Canonical input-result event. */
  result_event_seq: bigint
  /** Zero-based matched row. */
  row: number
  /** Zero-based first matched column. */
  start_column: number
  /** Zero-based column that was clicked. */
  target_column: number
  /** Exact matched text. */
  text: string
}

/** The actor encoded the request against live modes and wrote these exact bytes. */
export interface PointerOutcomeApplied {
  /** Canonical positions after application. */
  boundaries: ActorBoundaries
  /** Exact bytes the actor wrote. */
  bytes: Base64Bytes
  /** Number of bytes written completely. */
  bytes_written: bigint
  /** Semantic input category. */
  input_kind: InputKind
  /** Canonical input-intent event. */
  intent_event_seq: bigint
  kind: "applied"
  /** Identity of the request, also its canonical operation identity. */
  request_id: string
  /** Canonical input-result event. */
  result_event_seq: bigint
}

/** The application does not report this input, so the actor correctly wrote nothing. */
export interface PointerOutcomeNoop {
  /** Canonical positions at evaluation. */
  boundaries: ActorBoundaries
  /** Semantic input category. */
  input_kind: InputKind
  kind: "noop"
  /** Stable explanation, such as `pointer_has_no_output` or `pointer_reporting_disabled`. */
  reason: string
  /** Identity of the request. */
  request_id: string
  /** BLAKE3 of canonical JSON for the evaluated terminal modes. */
  terminal_modes_hash: string
}

/** Postcondition result of one action without its frame, which bindings expose separately. */
export type PostconditionSummary = PostconditionSummaryNone | PostconditionSummaryIdle | PostconditionSummaryScreen

/** An idle postcondition was requested. */
export interface PostconditionSummaryIdle {
  kind: "idle"
  /** Whether the screen settled. */
  matched: boolean
  /** Settlement classification. */
  quality: StableCaptureQuality
  /** Semantic state identity of the returned frame, when one was returned. */
  state_hash?: string | null
}

/** No postcondition was requested. */
export interface PostconditionSummaryNone {
  kind: "none"
  /** Whether the action matched. */
  matched: boolean
  /** Semantic state identity of the returned frame, when one was returned. */
  state_hash?: string | null
}

/** A screen predicate postcondition was requested. */
export interface PostconditionSummaryScreen {
  kind: "screen"
  /** Whether the predicate matched. */
  matched: boolean
  /** Completion classification. */
  quality: ScreenPostconditionQuality
  /** Semantic state identity of the returned frame, when one was returned. */
  state_hash?: string | null
}

/** Delivery codec and pixel-format contract. */
export interface PresentationDelivery {
  /** Stable delivery codec name. */
  codec: string
  /** libx264 constant-rate-factor quality. */
  crf?: number
  /** libx264 speed/compression preset. */
  encoder_preset?: string
  /** Stable output pixel format. */
  pixel_format: string
}

/** Canvas layout for caption presets. */
export interface PresentationLayout {
  /** Optional caller-owned branding label. */
  branding?: string | null
  /** Caption placement, with footer preserving terminal pixels. */
  caption: CaptionLayout
}

/** Dry-run output binding a normalized edit to immutable visual evidence. */
export interface PresentationPlan {
  /** Annotation tracks normalized onto the output frame timeline. */
  annotations: Array<NormalizedAnnotation>
  /** Stable change-overlay implementation and truth description. */
  change_algorithm?: string | null
  /** Semantic deltas scheduled onto mapped presentation frames. */
  change_frames: Array<NormalizedChangeFrame>
  /** Explicit per-frame change overlay policy. */
  change_visualization?: ChangeVisualization | null
  /** Caller-ordered resolved clips. */
  clips: Array<NormalizedClip>
  /** Cursor regions nominally masked from presentation pixels. */
  cursor_masks: Array<CursorMaskFrame>
  /** Delivery encoding contract. */
  delivery: PresentationDelivery
  /** BLAKE3 of canonical input edit-plan JSON. */
  edit_plan_hash: string
  /** Native capture or generated semantic source frame rate. */
  fps: number
  /** Presentation canvas and caption policy. */
  layout: PresentationLayout
  /** Canonical markers mapped onto native source frames. */
  markers: Array<MarkerRecord>
  /** Total output frames after retiming and holds. */
  output_frame_count: bigint
  /** Final presentation canvas height after footer and transforms. */
  output_height: number
  /** Final presentation canvas width after side panels and transforms. */
  output_width: number
  /** Effective user-facing render settings retained from the facade. */
  render_config?: VideoRenderConfig | null
  /** Explicit immutable visual source; native remains the default request. */
  source: PresentationSource
  /** Measured native or deterministic semantic grid and cell geometry. */
  source_geometry: TerminalGeometryReport
  /** Visual source height in pixels. */
  source_height: number
  /** Visual source width in pixels. */
  source_width: number
  /** Validated caller-authored subtitle resources. */
  subtitles: Array<NormalizedSubtitle>
  /** Ordered visibly labeled pixel transforms. */
  transforms: Array<PixelTransform>
  /** Presentation format version. */
  version: number
}

/** Immutable visual source selected for one presentation plan. */
export type PresentationSource = PresentationSourceNative | PresentationSourceSemantic

/** Actual pixels from a verified native Ghostty capture. */
export interface PresentationSourceNative {
  kind: "native"
  /** Lossless native source video. */
  video: string
  /** BLAKE3 of the native source video. */
  video_hash: string
}

/** Deterministic generated pixels from canonical semantic replay. */
export interface PresentationSourceSemantic {
  kind: "semantic"
  /** Complete semantic source plan. */
  plan: SemanticVideoSourcePlan
}

/** Bounded storyboard and low-resolution preview policy. */
export interface PreviewOptions {
  /** Select only frames carrying the configured change visualization. */
  change_only: boolean
  /** Storyboard tile columns. */
  columns: number
  /** Maximum selected storyboard frames. */
  max_frames: number
  /** Low-resolution preview width. */
  preview_width: number
  /** Width of each storyboard tile. */
  tile_width: number
}

/** Safely supported process-group signals. */
export type ProcessSignal = "interrupt" | "terminate" | "hangup" | "user1" | "user2"

/** State of an OSC 9;4 progress report. */
export type ProgressState = "remove" | "set" | "error" | "indeterminate" | "pause" | "other"

/** Bounded projected canonical events. `has_more` concerns this finite snapshot only. */
export interface ProjectedEventPage {
  /** Next scanned position. Filters may advance an empty page. */
  cursor: EventPageCursor
  /** Ordered matching projections with hashes, timestamps, and optional byte summaries. */
  events: Array<EventProjection>
  /** A matching complete record remains in the snapshot. */
  has_more: boolean
  /** An incomplete trailing record was excluded from this live snapshot. */
  incomplete_tail: boolean
}

/** Requested terminating-frame projection in host form. */
export interface ProjectionSpec {
  /** Requested detail; `text` when absent. */
  detail?: FrameDetail | null
  /** Exact half-open region; the complete viewport when absent. */
  region?: ScreenRegion | null
}

/** Which prompt an OSC 133 prompt start draws. */
export type PromptKind = "primary" | "right" | "continuation" | "secondary" | "other"

/** How one selected provenance value is represented. */
export type ProvenanceCapture = "value" | "hash" | "name" | "not_captured"

/** Bounded normalized query evidence. */
export interface QueryDescription {
  /** Human-readable canonical query. */
  text: string
  /** Whether it was truncated at a Unicode boundary. */
  truncated: boolean
}

/** Pure query and strict-result failures. */
export type QueryError = QueryErrorInvalidQuery | QueryErrorNoMatch | QueryErrorMultipleMatches | QueryErrorIndexOutOfBounds | QueryErrorQueryResourceLimit | QueryErrorInsufficientFrameDetail

/** A zero-based explicit index was absent. */
export interface QueryErrorIndexOutOfBounds {
  /** Exact total count. */
  count: bigint
  /** Exact rich-frame projection identity. */
  frame_hash: string
  /** Requested index. */
  index: bigint
  kind: "index_out_of_bounds"
  /** Bounded normalized query. */
  query: QueryDescription
  /** Exact semantic state identity. */
  state_hash: string
}

/** The frame detail could not be losslessly evaluated. */
export interface QueryErrorInsufficientFrameDetail {
  kind: "insufficient_frame_detail"
  /** Bounded reason. */
  reason: string
}

/** Empty, zero-width, conflicting, or otherwise ambiguous query input. */
export interface QueryErrorInvalidQuery {
  kind: "invalid_query"
  /** Bounded reason. */
  reason: string
}

/** A strict operation found multiple matches. */
export interface QueryErrorMultipleMatches {
  /** Exact total count. */
  count: bigint
  /** Exact rich-frame projection identity. */
  frame_hash: string
  kind: "multiple_matches"
  /** Bounded normalized query. */
  query: QueryDescription
  /** At most the first bounded ranges. */
  ranges: Array<MatchRange>
  /** Exact semantic state identity. */
  state_hash: string
  /** Whether additional ranges were omitted. */
  truncated: boolean
}

/** A strict operation found no match. */
export interface QueryErrorNoMatch {
  /** Exact rich-frame projection identity. */
  frame_hash: string
  kind: "no_match"
  /** Bounded normalized query. */
  query: QueryDescription
  /** Exact semantic state identity. */
  state_hash: string
}

/** A bounded query result exceeded its public resource ceiling. */
export interface QueryErrorQueryResourceLimit {
  kind: "query_resource_limit"
  /** Public maximum. */
  maximum: bigint
  /** Exact number that would have been returned. */
  requested: bigint
}

/** Exact positive playback speed numerator/denominator. */
export interface RationalSpeed {
  /** Source-time multiplier denominator. */
  denominator: number
  /** Source-time multiplier numerator. */
  numerator: number
}

/** Reaction required before an idle interval, in host millisecond units. */
export interface ReactionSpec {
  /** Presented-state hash the screen must leave; absent means the state at admission. */
  changed_from?: string | null
  /** How long to wait for a reaction. */
  timeout_ms?: bigint | null
}

/** First invalid tail encountered during prefix recovery. */
export interface RecoveryIssue {
  /** One-based source line where validation failed. */
  line: bigint
  /** Stable human-readable validation failure. */
  reason: string
}

/** Evidence produced by non-destructive event-log prefix recovery. */
export interface RecoveryReport {
  /** Invalid tail, if one was discarded. */
  discarded_tail?: RecoveryIssue | null
  /** Operations with a durable intent but uncertain result. */
  pending_operations: Array<PendingOperationReport>
  /** New owner-only recovered path. */
  recovered: ExactPath
  /** Original source path. */
  source: ExactPath
  /** Number of integrity-checked events retained. */
  valid_events: bigint
}

/** One friendly name bound to an immutable canonical session identity. */
export interface RegistryEntry {
  /** Validated friendly alias. */
  alias: string
  /** Exact canonical session-directory path bytes. */
  canonical_session: Base64Bytes
  /** Control protocol expected by the registering build. */
  control_protocol_version: number
  /** Stable entry format identifier. */
  format: string
  /** Recorder PID when a supervisor supplied one. */
  recorder_pid?: number | null
  /** Linux process start-time ticks paired with `recorder_pid` against PID reuse. */
  recorder_process_start_ticks?: bigint | null
  /** Registry publication time from the local system clock. */
  registered_unix_ns: bigint
  /** Immutable session ID read from `session.json` during registration. */
  session_id: string
  /** Entry schema version. */
  version: number
}

/** One registry listing row with evidence-derived session summary. */
export interface RegistryListEntry {
  /** Friendly alias. */
  alias: string
  /** Exact canonical session path bytes. */
  canonical_session: Base64Bytes
  /** Effective command bytes when the manifest is readable. */
  command?: Array<Base64Bytes> | null
  /** Focused diagnostic for unhealthy entries. */
  diagnostic?: string | null
  /** Initial grid and pixel geometry when the manifest is readable. */
  geometry?: SessionGeometry | null
  /** Current derived lifecycle classification. */
  lifecycle: SessionPhase
  /** Registry publication time. */
  registered_unix_ns: bigint
  /** Immutable registered session ID. */
  session_id: string
  /** Session wall-clock origin when the manifest is readable. */
  started_unix_ns?: bigint | null
  /** Effective working directory when the manifest is readable. */
  working_directory?: Base64Bytes | null
}

/** A named session path paired with the registry's evidence-derived lifecycle. */
export interface RegistrySessionResolution {
  /** Focused explanation for incomplete or unhealthy states. */
  diagnostic?: string | null
  /** Current lifecycle classification. */
  lifecycle: SessionPhase
  /** Exact registered or pending session path. */
  session: ExactPath
}

/** Result of explicitly deleting disposable replay checkpoints. */
export interface ReplayCachePruneReport {
  /** Cache directory considered. */
  directory: ExactPath
  /** Inventory remaining after pruning. */
  remaining: ReplayCacheStatus
  /** Compressed bytes removed. */
  removed_bytes: bigint
  /** Files removed. */
  removed_entries: bigint
}

/** Bounded disposable replay-cache inventory for one canonical session. */
export interface ReplayCacheStatus {
  /** Total compressed bytes retained. */
  compressed_bytes: bigint
  /** Cache directory inside the canonical session. */
  directory: ExactPath
  /** Number of checkpoint files currently retained. */
  entries: bigint
  /** Automatic per-session compressed-byte limit. */
  maximum_compressed_bytes: bigint
  /** Automatic per-session checkpoint limit. */
  maximum_entries: bigint
}

/** Canonical resource-limit kinds with implemented completion semantics. */
export type ResourceLimitKind = "output_bytes" | "max_duration" | "idle_timeout"

/** Resource-limit trigger retained in immutable completion evidence. */
export interface ResourceLimitOutcome {
  /** Stable limit kind. */
  kind: ResourceLimitKind
  /** Configured trigger threshold. */
  limit: bigint
  /** Exact observed count when the host triggered termination. */
  observed: bigint
}

/** Exact allocated paths and proven successor launch/readiness facts, not rollback claims. */
export interface RestartAttempt {
  /** True only after spawning the actual Child. */
  launched: boolean
  /** Last actual readiness observation, absent before observing it. */
  readiness?: SuccessorReadiness | null
  /** Authoritative session-directory bytes. */
  session: Base64Bytes
  /** Authoritative stderr-log path bytes. */
  stderr_log: Base64Bytes
  /** Authoritative stdout-log path bytes. */
  stdout_log: Base64Bytes
}

/** Typed successor configuration. Absent fields keep predecessor values; clear flags explicitly remove optional limits. This is a recorder-launch DTO, not a persisted evidence format. */
export interface RestartConfiguration {
  /** Explicitly permit an intentionally changed environment. */
  allow_environment_change?: boolean
  /** New cell height, absent to keep. */
  cell_height?: number | null
  /** New cell width, absent to keep. */
  cell_width?: number | null
  /** Clear the inactivity limit. */
  clear_idle_timeout?: boolean
  /** Clear the wall-clock limit. */
  clear_max_duration?: boolean
  /** Clear the output budget rather than keeping it. */
  clear_output_byte_budget?: boolean
  /** New columns, absent to keep. */
  cols?: number | null
  /** New exact executable/arguments; absent to keep. */
  command?: Array<Base64Bytes> | null
  /** New exact working directory, absent to keep. */
  cwd?: Base64Bytes | null
  /** New durability mode, absent to keep. */
  durability?: RestartDurability | null
  /** Bounded durability interval override. */
  durability_max_interval_ms?: bigint | null
  /** Bounded durability byte override. */
  durability_max_uncommitted_bytes?: bigint | null
  /** New inactivity limit. */
  idle_timeout_ms?: bigint | null
  /** New wall-clock duration limit. */
  max_duration_ms?: bigint | null
  /** New output budget. */
  output_byte_budget?: bigint | null
  /** New provenance collection mode. */
  provenance?: RestartProvenance | null
  /** Explicit provenance environment rules. */
  provenance_environment?: Array<[string, ProvenanceCapture]>
  /** New rows, absent to keep. */
  rows?: number | null
  /** New TERM, absent to keep. */
  term?: string | null
}

/** Recorder restart durability override. Pascal-case wire names preserve the existing CLI handoff. */
export type RestartDurability = "Strict" | "Bounded" | "Throughput"

/** Proven restart progress. No alias rollback or deduplicated replay is promised. */
export interface RestartFailureFacts {
  /** Alias used for this orchestration. */
  alias: string
  /** Last reached phase. */
  phase: RestartFailurePhase
  /** Actual immutable completion returned by predecessor stop, never synthesized. */
  predecessor_completion?: SessionCompletion | null
  /** Exact resolved predecessor path, when resolution succeeded. */
  predecessor_path?: Base64Bytes | null
  /** Allocated successor/orphan locations and proven admission facts. */
  successor?: RestartAttempt | null
}

/** Where a restart stopped; workflow phases are not input atoms. */
export type RestartFailurePhase = "resolve_predecessor" | "stopping_predecessor" | "predecessor_completed" | "successor_allocated" | "successor_launched" | "successor_ready" | "lineage_validated" | "alias_retarget"

/** Recorder restart provenance override, preserving existing internal wire names. */
export type RestartProvenance = "Minimal" | "Reproducible" | "Custom"

/** Shared restart result, retaining the predecessor and compare-and-retargeted successor. */
export interface RestartReport {
  /** Alias compare-and-retargeted after validated readiness and lineage. */
  alias: string
  /** Preserved predecessor binding. */
  predecessor: RegistryEntry
  /** True only after successful retargeting. */
  ready: boolean
  /** Exact owner-only stderr log path. */
  stderr_log: Base64Bytes
  /** Exact owner-only stdout log path. */
  stdout_log: Base64Bytes
  /** New binding. */
  successor: RegistryEntry
}

/** What a caller may safely do after a failed request. */
export type Retry = "new_request" | "same_request_id" | "inspect_first" | "do_not_retry"

/** Manifest for one explicit reviewer state export. */
export interface ReviewExportManifest {
  /** Immutable source session. */
  canonical_session: ExactPath
  /** BLAKE3 of `screen.txt`. */
  screen_text_blake3: string
  /** Canonical source head anchoring the export. */
  source_final_event_hash: string
  /** Exact semantic state hash. */
  state_hash: string
  /** Exact exported semantic state identifier. */
  state_id: bigint
  /** BLAKE3 of `state.json`. */
  state_json_blake3: string
  /** Reviewer export contract version. */
  version: number
}

/** One semantic viewport text match with a copyable canonical selector. */
export interface ReviewSearchMatch {
  /** Canonical event sequence associated with the state. */
  event_seq?: bigint | null
  /** Whether the state was presentation-eligible. */
  presented: boolean
  /** Ready-to-run selector arguments. */
  selector: string
  /** Exact semantic state hash. */
  state_hash: string
  /** Exact semantic state identifier. */
  state_id: bigint
}

/** Stable noninteractive reviewer text-search result. */
export interface ReviewSearchResult {
  /** Immutable completed session directory. */
  canonical_session: ExactPath
  /** Ordered semantic state matches. */
  matches: Array<ReviewSearchMatch>
  /** Exact viewport substring query. */
  query: string
}

/** Noninteractive reviewer source summary. */
export interface ReviewSummary {
  /** Immutable completed session directory. */
  canonical_session: ExactPath
  /** Final semantic state hash. */
  final_state_hash: string
  /** Number of canonical markers. */
  marker_count: bigint
  /** Number of honest direct or scheduled native correlations. */
  native_correlation_count: bigint
  /** Number of presentation-eligible states. */
  presented_state_count: bigint
  /** Safety boundary used for viewport rendering. */
  rendering_policy: string
  /** Number of semantic boundary states. */
  state_count: bigint
}

/** RGB color independent of libghostty's unstable ABI. */
export interface Rgb {
  /** Blue channel. */
  b: number
  /** Green channel. */
  g: number
  /** Red channel. */
  r: number
}

/** Changed cells and native wrap facts for one viewport row. */
export interface RowChange {
  /** Complete semantic cell changes. */
  cells: Array<CellChange>
  /** Zero-based row. */
  row: number
  /** Earlier/later soft-wrap fact, including row absence at either endpoint. */
  soft_wrapped?: ValueChange8 | null
  /** Earlier/later wrap-continuation fact, including row absence at either endpoint. */
  wrap_continuation?: ValueChange8 | null
}

/** Fine-grained changes for one viewport row. */
export interface RowDiff {
  /** Cells whose complete semantic value changed. */
  cells: Array<CellState>
  /** Zero-based row. */
  row: number
  /** Whether the row ends in a native soft wrap. */
  soft_wrapped: boolean
  /** Whether the row continues a native soft wrap from its predecessor. */
  wrap_continuation: boolean
}

/** Contiguous block of complete rows found at a different position in the later state. */
export interface RowMove {
  /** Number of rows in the block. */
  count: number
  /** First row of the block in the earlier state. */
  from_row: number
  /** First row of the block in the later state. */
  to_row: number
}

/** Semantic file recipe supported by `save`. */
export type SaveFormat = "txt" | "json" | "deep-json" | "vt" | "html" | "svg" | "png"

/** Manifest binding every saved representation to one source state. */
export interface SaveManifest {
  /** Exact canonical session path bytes. */
  canonical_session: Base64Bytes
  /** Manifested representations. */
  files: Array<SavedFile>
  /** Stable manifest format. */
  format: string
  /** Exact deterministic resources when semantic images are present. */
  semantic_image_resources: Array<SemanticImageResource>
  /** Explicit sensitivity classification. */
  sensitivity: string
  /** Canonical session identity. */
  session_id: string
  /** Final canonical event hash. */
  source_final_event_hash: string
  /** Resolved complete semantic state hash. */
  state_hash: string
  /** Resolved state identifier. */
  state_id: bigint
  /** Requested semantic point. */
  state_point: StatePoint
  /** Manifest version. */
  version: number
}

/** Atomic save result. */
export interface SaveOutcome {
  /** Published directory. */
  directory: ExactPath
  /** Published manifest. */
  manifest: SaveManifest
}

/** One manifested saved file. */
export interface SavedFile {
  /** BLAKE3 of exact file bytes. */
  blake3: string
  /** Exact byte count. */
  byte_count: bigint
  /** Media type. */
  media_type: string
  /** Relative filename. */
  path: string
  /** Stable renderer identity, when applicable. */
  renderer?: string | null
  /** Rendering truth layer, when this member is a rendering. */
  truth_layer?: string | null
}

/** One fresh-session attempt retained within a scenario suite result. */
export interface ScenarioAttemptReport {
  /** One-based attempt number. */
  attempt: number
  /** Wall-clock duration including cleanup and evidence publication, in milliseconds. */
  duration_ms: bigint
  /** Complete fresh-session result. */
  report: ScenarioReport
}

/** Every attempt for one selected scenario document. */
export interface ScenarioCaseReport {
  /** Ordered fresh-session attempts, including failed attempts before a pass. */
  attempts: Array<ScenarioAttemptReport>
  /** Input scenario document path. */
  scenario: ExactPath
  /** Factual verdict, independent of exit policy. */
  status: ScenarioCaseStatus
}

/** Factual verdict for a selected case. */
export type ScenarioCaseStatus = "passed" | "flaky" | "failed"

/** Stable failure context for one scenario step. */
export interface ScenarioFailure {
  /** Safe diagnostic without child environment values. */
  message: string
  /** Caller-owned mutation identifiers allocated before the failure. */
  request_ids: Array<string>
  /** Zero-based failed step index. */
  step_index: bigint
  /** Stable scenario step kind. */
  step_kind: string
}

/** Safe structured infrastructure diagnostic. No child output or environment values. */
export interface ScenarioInfrastructureError {
  /** Stable machine-readable error code. */
  code: string
  /** Safe bounded explanation. */
  message: string
  /** Lifecycle or reporting phase. */
  phase: string
}

/** Machine-readable scenario outcome. */
export interface ScenarioReport {
  /** Optional evidence derivations that could not be produced. */
  artifact_warnings: Array<string>
  /** Owner-only canonical evidence path, absent for prelaunch failures. */
  canonical_session?: ExactPath | null
  /** First failure, if any. */
  failure?: ScenarioFailure | null
  /** Atomic failure evidence directory, only after successful publication. */
  failure_artifacts?: ExactPath | null
  /** Ordered required infrastructure failures, including secondary cleanup errors. */
  infrastructure_errors: Array<ScenarioInfrastructureError>
  /** Scenario name. */
  name: string
  /** Immutable canonical session identity, absent when launch never returned a session. */
  session_id?: string | null
  /** Overall disposition. */
  status: ScenarioStatus
  /** Successfully completed steps. */
  steps: Array<ScenarioStepResult>
  /** Scenario report contract version (independent of documents). */
  version: number
}

/** Overall scenario disposition. */
export type ScenarioStatus = "passed" | "failed" | "error"

/** One executed step and its canonical provenance. */
export interface ScenarioStepResult {
  /** Zero-based step index. */
  index: bigint
  /** Stable scenario step kind. */
  kind: string
  /** Caller-owned mutation request identifiers used by this step. */
  request_ids: Array<string>
  /** Matched presented state hash when applicable. */
  state_hash?: string | null
}

/** Machine-readable sharded, bounded-concurrency scenario suite result. */
export interface ScenarioSuiteReport {
  /** Ordered selected scenarios and their retained attempts. */
  cases: Array<ScenarioCaseReport>
  /** Wall-clock suite duration in milliseconds (not summed worker time). */
  duration_ms: bigint
  /** Effective policy: recovered expectation failures make CI fail when true. */
  fail_on_flaky: boolean
  /** Ordered suite-level reporting/orchestration errors. */
  infrastructure_errors: Array<ScenarioInfrastructureError>
  /** Requested maximum worker count. */
  jobs: bigint
  /** Optional total shard count. */
  shard_count?: bigint | null
  /** Optional zero-based shard index. */
  shard_index?: bigint | null
  /** Overall disposition. */
  status: ScenarioSuiteStatus
  /** Suite result contract version. */
  version: number
}

/** Overall multi-scenario disposition. */
export type ScenarioSuiteStatus = "passed" | "flaky" | "failed"

/** Complete appearance shared by an exact cell or a lossless span. */
export interface ScreenAppearance {
  /** Resolved explicit background before default fallback. */
  resolved_background?: Rgb | null
  /** Resolved explicit foreground before default fallback. */
  resolved_foreground?: Rgb | null
  /** Selection membership. */
  selected: boolean
  /** Authoritative source style and color selectors. */
  style: CellStyle
}

/** Exact projected terminal cell. */
export interface ScreenCell {
  /** Complete semantic appearance. */
  appearance: ScreenAppearance
  /** Absolute viewport column. */
  column: number
  /** Complete grapheme cluster, empty for a blank/spacer. */
  grapheme: string
  /** Absolute viewport row. */
  row: number
  /** Native Ghostty width role. */
  wide_kind: WideKind
}

/** Cell-detail row. */
export interface ScreenCellRow {
  /** Exact ordered cells. */
  cells: Array<ScreenCell>
  /** Absolute row. */
  row: number
  /** Whether this row ends in a native soft wrap. */
  soft_wrapped: boolean
  /** Deterministic convenience text. */
  text: string
  /** Whether this row continues a native soft wrap. */
  wrap_continuation: boolean
}

/** Presented-screen changes between two frames of the same projection. */
export interface ScreenChanges {
  /** State hash of the later frame. */
  after_state_hash: string
  /** State hash of the earlier frame. */
  before_state_hash: string
  /** Cursor change. */
  cursor?: ValueChange | null
  /** Geometry change as columns and rows. */
  geometry?: ValueChange2 | null
  /** Terminal mode change. */
  modes?: ValueChange3 | null
  /** Blocks of complete rows that moved. */
  moves: Array<RowMove>
  /** Rows that differ at the same position and hold no moved content. */
  rows: Array<ScreenRowChange>
}

/** Versioned, bounded projection of one exact semantic state. */
export interface ScreenFrame {
  /** Settlement/provenance label. */
  capture_quality: CaptureQuality
  /** Current/default colors and complete palette. */
  colors: ColorState
  /** Complete cursor state. */
  cursor: CursorState
  /** Structurally matched detail payload. */
  data: ScreenFrameData
  /** Projection identity in the `termtrace-screen-frame-v1` hash domain. */
  frame_hash: string
  /** Full viewport geometry. */
  geometry: ScreenGeometry
  /** Complete relevant terminal modes. */
  modes: ModeState
  /** Whether this state was presentation eligible. */
  presented: boolean
  /** Validated requested region. */
  requested_region: ScreenRegion
  /** Actually returned region; equal to requested in version 1. */
  returned_region: ScreenRegion
  /** Exact source occurrence. */
  source: ScreenFrameSource
  /** Complete semantic v5 state identity. */
  state_hash: string
  /** Projection format version. */
  version: number
}

/** Structurally discriminated rich row payload. */
export type ScreenFrameData = ScreenFrameDataNone | ScreenFrameDataText | ScreenFrameDataSpans | ScreenFrameDataCells

/** Exact cells. */
export interface ScreenFrameDataCells {
  detail: "cells"
  /** Ordered projected rows. */
  rows: Array<ScreenCellRow>
}

/** Observation metadata only; no cell or text payload. */
export interface ScreenFrameDataNone {
  detail: "none"
}

/** Lossless appearance spans. */
export interface ScreenFrameDataSpans {
  detail: "spans"
  /** Ordered projected rows. */
  rows: Array<ScreenSpanRow>
}

/** Convenience text and authoritative wrap facts. */
export interface ScreenFrameDataText {
  detail: "text"
  /** Ordered projected rows. */
  rows: Array<ScreenTextRow>
}

/** Bounded context for a rejected rich screen projection. */
export interface ScreenFrameFailureDetails {
  /** Full terminal columns when geometry was available. */
  cols?: number | null
  /** Requested projection detail when known. */
  detail?: FrameDetail | null
  /** `cell_count`, `serialized_bytes`, or `response_bytes` for limit failures. */
  limit_kind?: string | null
  /** Maximum permitted amount for a limit failure. */
  maximum?: bigint | null
  /** Requested region when supplied. */
  region?: ScreenRegion | null
  /** Typed invalid-region reason when validation failed. */
  region_reason?: InvalidRegionReason | null
  /** Requested measured amount for a limit failure. */
  requested?: bigint | null
  /** Full terminal rows when geometry was available. */
  rows?: number | null
}

/** Exact provenance of the projected semantic state. */
export type ScreenFrameSource = ScreenFrameSourceActor | ScreenFrameSourceResolved

/** State selected while the live actor still owned exclusivity. */
export interface ScreenFrameSourceActor {
  /** Durable and actor-observed canonical boundaries. */
  boundaries: ActorBoundaries
  /** Actor observation time in session-monotonic nanoseconds. */
  observed_monotonic_ns: bigint
  /** Canonical session identity. */
  session_id: string
  source: "actor"
  /** Stream position of the presented state, as replay of the session numbers it. */
  state_id: bigint
}

/** State selected through a completed or explicit semantic resolution. */
export interface ScreenFrameSourceResolved {
  /** Exact validated source boundary. */
  boundary: CanonicalBoundary
  /** Responsible canonical event sequence when known. */
  event_seq?: bigint | null
  /** Integrity relationship to canonical evidence. */
  integrity: ObservationIntegrity
  /** Session-monotonic timestamp when known. */
  monotonic_ns?: bigint | null
  /** Resolution path used to select the state. */
  resolution: ObservationResolution
  /** Canonical session identity when available. */
  session_id?: string | null
  source: "resolved"
  /** Whether canonical evidence completed durably. */
  source_completed: boolean
  /** Source category. */
  source_kind: ObservationSourceKind
  /** Stream position of the state when known. */
  state_id?: bigint | null
  /** Requested state selector. */
  state_point: StatePoint
}

/** Complete terminal geometry, independent of the returned region. */
export interface ScreenGeometry {
  /** Viewport columns. */
  cols: number
  /** Viewport rows. */
  rows: number
}

/** Stable terminal-mode selectors available to screen predicates. */
export type ScreenMode = ScreenModeAlternateScreen | ScreenModeBracketedPaste | ScreenModeWraparound | ScreenModeSynchronizedOutput | ScreenModeAnsi | ScreenModeDec

/** Any alternate-screen mode. */
export interface ScreenModeAlternateScreen {
  mode: "alternate_screen"
}

/** Numeric ANSI mode. */
export interface ScreenModeAnsi {
  mode: "ansi"
  /** Mode number. */
  number: number
}

/** DEC bracketed-paste mode 2004. */
export interface ScreenModeBracketedPaste {
  mode: "bracketed_paste"
}

/** Numeric DEC-private mode. */
export interface ScreenModeDec {
  mode: "dec"
  /** Mode number. */
  number: number
}

/** DEC synchronized-output mode 2026. */
export interface ScreenModeSynchronizedOutput {
  mode: "synchronized_output"
}

/** DEC auto-wrap mode 7. */
export interface ScreenModeWraparound {
  mode: "wraparound"
}

/** Why an exact action screen postcondition returned. */
export type ScreenPostconditionQuality = "matched" | "matched_unsettled" | "child_completed" | "output_closed" | "deadline"

/** Exact predicate evaluated against the latest presented viewport. */
export type ScreenPredicate = ScreenPredicateContains | ScreenPredicateAbsent | ScreenPredicateMatchesRegex | ScreenPredicateExcludesRegex | ScreenPredicateRowContains | ScreenPredicateRowAbsent | ScreenPredicateRegionContains | ScreenPredicateRegionAbsent | ScreenPredicateCursor | ScreenPredicateMode | ScreenPredicateStateHash | ScreenPredicateAll | ScreenPredicateAny

/** Viewport text excludes an exact UTF-8 substring. */
export interface ScreenPredicateAbsent {
  predicate: "absent"
  /** Exact UTF-8 substring. */
  text: string
}

/** Every nested predicate must match. */
export interface ScreenPredicateAll {
  predicate: "all"
  /** Bounded non-empty predicate list. */
  predicates: Array<ScreenPredicate>
}

/** At least one nested predicate must match. */
export interface ScreenPredicateAny {
  predicate: "any"
  /** Bounded non-empty predicate list. */
  predicates: Array<ScreenPredicate>
}

/** Viewport text contains an exact UTF-8 substring. */
export interface ScreenPredicateContains {
  predicate: "contains"
  /** Exact UTF-8 substring. */
  text: string
}

/** Cursor properties all equal the supplied values. */
export interface ScreenPredicateCursor {
  /** Optional zero-based viewport column. */
  column?: number | null
  predicate: "cursor"
  /** Optional zero-based viewport row. */
  row?: number | null
  /** Optional cursor shape requirement. */
  shape?: CursorStyle | null
  /** Optional cursor visibility requirement. */
  visible?: boolean | null
}

/** Viewport text does not match a bounded linear-time regular expression. */
export interface ScreenPredicateExcludesRegex {
  /** Rust-regex pattern, limited by the control service. */
  pattern: string
  predicate: "excludes_regex"
}

/** Viewport text matches a bounded linear-time regular expression. */
export interface ScreenPredicateMatchesRegex {
  /** Rust-regex pattern, limited by the control service. */
  pattern: string
  predicate: "matches_regex"
}

/** One stable terminal mode equals the requested value. */
export interface ScreenPredicateMode {
  /** Required set/reset value. */
  enabled: boolean
  /** Stable mode selector. */
  mode: ScreenMode
  predicate: "mode"
}

/** A half-open cell region excludes exact UTF-8 text. */
export interface ScreenPredicateRegionAbsent {
  predicate: "region_absent"
  /** Region bounds in viewport cells. */
  region: ScreenRegion
  /** Exact UTF-8 substring. */
  text: string
}

/** A half-open cell region contains exact UTF-8 text. */
export interface ScreenPredicateRegionContains {
  predicate: "region_contains"
  /** Region bounds in viewport cells. */
  region: ScreenRegion
  /** Exact UTF-8 substring. */
  text: string
}

/** One zero-based row excludes exact UTF-8 text. */
export interface ScreenPredicateRowAbsent {
  predicate: "row_absent"
  /** Zero-based viewport row. */
  row: number
  /** Exact UTF-8 substring. */
  text: string
}

/** One zero-based row contains exact UTF-8 text. */
export interface ScreenPredicateRowContains {
  predicate: "row_contains"
  /** Zero-based viewport row. */
  row: number
  /** Exact UTF-8 substring. */
  text: string
}

/** Presented state hash equals or differs from a supplied BLAKE3 hash. */
export interface ScreenPredicateStateHash {
  /** Equality relationship to require. */
  comparison: StateHashComparison
  /** Lowercase BLAKE3 state hash. */
  hash: string
  predicate: "state_hash"
}

/** Absolute zero-based half-open terminal region. */
export interface ScreenRegion {
  /** Exclusive bottom row. */
  bottom: number
  /** Inclusive left column. */
  left: number
  /** Exclusive right column. */
  right: number
  /** Inclusive top row. */
  top: number
}

/** One changed row of a presented-screen frame. */
export interface ScreenRowChange {
  /** Later row text without trailing blanks, absent when the later frame has no such row. */
  after?: string | null
  /** Whether the text is unchanged and only appearance or wrap facts differ. */
  appearance_only: boolean
  /** Earlier row text without trailing blanks, absent when the earlier frame had no such row. */
  before?: string | null
  /** Absolute viewport row. */
  row: number
}

/** Lossless run of cells with identical complete appearance. */
export interface ScreenSpan {
  /** Appearance common to every covered cell. */
  appearance: ScreenAppearance
  /** Number of terminal columns covered. */
  cell_count: number
  /** Exact grapheme/width map in column order. */
  cells: Array<SpanCell>
  /** Exclusive absolute end column. */
  end_column: number
  /** Inclusive absolute start column. */
  start_column: number
  /** Deterministic convenience text for this run. */
  text: string
}

/** Span-detail row. */
export interface ScreenSpanRow {
  /** Absolute row. */
  row: number
  /** Whether this row ends in a native soft wrap. */
  soft_wrapped: boolean
  /** Lossless ordered appearance spans. */
  spans: Array<ScreenSpan>
  /** Deterministic convenience text. */
  text: string
  /** Whether this row continues a native soft wrap. */
  wrap_continuation: boolean
}

/** Text-detail row. */
export interface ScreenTextRow {
  /** Absolute row. */
  row: number
  /** Whether this row ends in a native soft wrap. */
  soft_wrapped: boolean
  /** Deterministic convenience text. */
  text: string
  /** Whether this row continues a native soft wrap. */
  wrap_continuation: boolean
}

/** Why an exact actor-side screen predicate operation returned. */
export type ScreenWaitQuality = "matched" | "matched_unsettled" | "child_completed" | "output_closed" | "deadline" | "completed"

/** Screen wait evidence with the derived satisfaction verdict and without its frame, which bindings attach as a native frame object. */
export interface ScreenWaitResult {
  /** First matching actor timestamp, when matched. */
  first_match_monotonic_ns?: bigint | null
  /** Whether the predicate matched before termination. */
  matched: boolean
  /** Actor timestamp of the returned frame. */
  observed_monotonic_ns: bigint
  /** Predicate evaluated by the actor. */
  predicate: ScreenPredicate
  /** Closed completion classification. */
  quality: ScreenWaitQuality
  /** Whether the predicate held on a frame that met the requested stability. */
  satisfied: boolean
  /** Requested stability duration. */
  stable_for_ns: bigint
  frame: FrameHandle
}

/** Portable member metadata. Canonical `ArtifactMember` serialization is unchanged. */
export interface SdkArtifactMember {
  /** Whole-file BLAKE3. */
  blake3: string
  /** Exact bytes in the complete file. */
  byte_length: DecimalU64
  /** MIME type. */
  media_type: string
  /** Relative filename. */
  path: ArtifactRelativePath
  /** Existing manifest relationship. */
  relationship: string
  /** Schema identifier, or explicit null. */
  schema_type: NullableString
  /** Existing sensitivity contract. */
  sensitivity: ArtifactSensitivity
}

/** Exact canonical boundary with portable wide integers. */
export interface SdkCanonicalBoundary {
  /** Prior event hash, or the event header hash at sequence zero. */
  event_hash: string
  /** Sequence assigned to the next event. */
  next_event_seq: DecimalU64
  /** Canonical PTY bytes through this boundary. */
  output_bytes: DecimalU64
}

/** One bounded failure that could not replace the body's primary failure. */
export interface SecondaryFailure {
  /** Bounded human-readable detail. */
  message: string
  /** Stable finalization phase. */
  phase: string
  /** Whether the original detail was truncated. */
  truncated: boolean
}

/** Complete semantic difference between two canonical state points. */
export interface SemanticDiff {
  /** Active SGR style change. */
  active_style?: ValueChange5 | null
  /** Whether any normalized semantic or presentation value changed. */
  changed: boolean
  /** Palette/default-color change. */
  colors?: ValueChange4 | null
  /** Cursor change. */
  cursor?: ValueChange | null
  /** Earlier endpoint identity and provenance. */
  from: DiffEndpoint
  /** Geometry change. */
  geometry?: ValueChange2 | null
  /** OSC metadata change. */
  metadata?: ValueChange6 | null
  /** Terminal mode change. */
  modes?: ValueChange3 | null
  /** Blocks of complete rows that moved; `rows` still compares cells at equal positions. */
  moves: Array<RowMove>
  /** Presentation-eligibility change. */
  presented?: ValueChange7 | null
  /** Minimal changed semantic cells grouped by row. */
  rows: Array<RowChange>
  /** Later endpoint identity and provenance. */
  to: DiffEndpoint
}

/** Exact identity of one bundled semantic font face. */
export interface SemanticFontResource {
  /** BLAKE3 of the decompressed TTF bytes. */
  decompressed_blake3: string
  /** Decompressed TTF byte count. */
  decompressed_size: bigint
  /** Termtrace-scoped family used by semantic renderers. */
  family: string
  /** Repository-relative source asset. */
  repository_source: string
  /** Stable face role. */
  role: SemanticFontRole
}

/** Stable role of one bundled semantic font face. */
export type SemanticFontRole = "regular" | "bold" | "italic" | "bold-italic" | "symbols-fallback"

/** Exact font and layout resources used by semantic image output. */
export interface SemanticImageResource {
  /** Background policy. */
  background_policy: string
  /** Fixed semantic cell height. */
  cell_height: number
  /** Fixed semantic cell width. */
  cell_width: number
  /** Deterministic missing-glyph policy. */
  fallback_policy: string
  /** Aggregate BLAKE3 identity of all decompressed bundled faces. */
  font_set_blake3: string
  /** Font size in semantic SVG units. */
  font_size: number
  /** Ordered bundled font resources in serialized role order. */
  fonts: Array<SemanticFontResource>
  /** Fixed outer padding. */
  padding: number
  /** Deterministic rasterization quality profile. */
  raster_profile: SemanticRasterProfile
  /** Symbols fallback baseline in semantic SVG units. */
  symbols_baseline: number
  /** Symbols fallback font size in semantic SVG units. */
  symbols_font_size: number
  /** Text baseline in semantic SVG units. */
  text_baseline: number
}

/** Bundled semantic fonts and fixed metrics. */
export type SemanticPreset = "bundled-v1"

/** Deterministic semantic rasterization quality profile. */
export type SemanticRasterProfile = "direct" | "supersampled-2x"

/** Fixed canvas used for every frame of one semantic video source. */
export interface SemanticVideoCanvas {
  /** Deterministic semantic cell height. */
  cell_height: number
  /** Deterministic semantic cell width. */
  cell_width: number
  /** Largest terminal column count in the source execution. */
  cols: number
  /** Encoded source height. */
  height: number
  /** Stable resize/canvas policy identifier. */
  policy: string
  /** Largest terminal row count in the source execution. */
  rows: number
  /** Encoded source width. */
  width: number
}

/** Immutable semantic source description resolved during presentation planning. */
export interface SemanticVideoSourcePlan {
  /** Fixed source canvas. */
  canvas: SemanticVideoCanvas
  /** Inclusive completed-session endpoint used for the final frame hold. */
  end_monotonic_ns: bigint
  /** Aggregate BLAKE3 identity of all decompressed bundled faces. */
  font_set_blake3: string
  /** Ordered bundled font resources in serialized role order. */
  fonts: Array<SemanticFontResource>
  /** Constant semantic source frame rate. */
  fps: number
  /** Exact source frame count, including one final semantic-state frame. */
  frame_count: bigint
  /** Deterministic rasterization quality profile. */
  raster_profile: SemanticRasterProfile
  /** Semantic renderer identity. */
  renderer: string
  /** Stable state-to-frame scheduling policy. */
  scheduling_policy: string
  /** Canonical session path. */
  session: ExactPath
  /** Canonical session identity. */
  session_id: string
  /** Terminal canonical event hash. */
  source_event_hash: string
  /** Inclusive source-time origin. */
  start_monotonic_ns: bigint
  /** Source-plan schema version. */
  version: number
}

/** Semantic renderer selection. */
export interface SemanticVisualProfile {
  /** Bundled semantic resource preset. */
  preset: SemanticPreset
}

/** Ordered mixed steps sent under one caller identity, in host form. */
export interface SendSpec {
  /** Presented-state guard of the first step only. */
  if_state?: string | null
  /** Human-readable intent retained with input operations. */
  intent?: string | null
  /** Delay between input atoms. */
  pace_ms?: bigint | null
  /** Terminating-frame projection. */
  projection?: ProjectionSpec | null
  /** Report presented-screen changes across the whole send. */
  report_changes?: boolean
  /** Identity of the whole send; later steps derive theirs from it. */
  request_id?: string | null
  /** Ordered steps; sending stops at the first failure. */
  steps: Array<SendStepSpec>
  /** Condition checked after the last step. */
  until?: ConditionSpec | null
  /** Report per-atom outcomes of input steps. */
  verbose?: boolean
}

/** One ordered step of [`SendSpec`]. */
export type SendStepSpec = SendStepSpecInput | SendStepSpecResize | SendStepSpecSignal | SendStepSpecMark | SendStepSpecPointer | SendStepSpecFocus | SendStepSpecActivate

/** Resolve visible text and click its center. */
export interface SendStepSpecActivate {
  step: "activate"
  /** Reject instead of a no-op when the application does not report the pointer. */
  strict?: boolean
  /** Exact row-contained visible text. */
  text: string
  /** Require exactly one match. */
  unique?: boolean
}

/** Report a focus transition. */
export interface SendStepSpecFocus {
  /** Whether the terminal gained focus. */
  focused: boolean
  step: "focus"
  /** Reject instead of a no-op when the application does not report focus. */
  strict?: boolean
}

/** A run of input atoms applied as one actor-ordered action. */
export interface SendStepSpecInput {
  /** Ordered atoms. */
  atoms: Array<AtomSpec>
  step: "input"
}

/** Append a canonical marker. */
export interface SendStepSpecMark {
  /** Non-empty label. */
  label: string
  step: "mark"
}

/** Apply one cell-addressed pointer operation. */
export interface SendStepSpecPointer {
  /** Zero-based column. */
  column: number
  /** Click, multi-click, drag, or scroll. */
  operation: PointerOperation
  /** Zero-based row. */
  row: number
  step: "pointer"
  /** Reject instead of a no-op when the application does not report the pointer. */
  strict?: boolean
}

/** Resize the child PTY; pixel dimensions derive from the default cell size. */
export interface SendStepSpecResize {
  /** Terminal columns. */
  cols: number
  /** Terminal rows. */
  rows: number
  step: "resize"
}

/** Send one allow-listed signal to the child process group. */
export interface SendStepSpecSignal {
  /** Signal to send. */
  signal: ProcessSignal
  step: "signal"
}

/** Immutable completion facts written after the final event is durable. */
export interface SessionCompletion {
  /** Recorder-observed persistence behavior and final sync result. */
  durability: DurabilityRecord
  /** Number of canonical events. */
  event_count: bigint
  /** BLAKE3 of the exact event-log header bytes. */
  event_header_hash: string
  /** Normal exit code when available. */
  exit_code?: number | null
  /** Fatal recorder failure, absent for normal child completion. */
  failure?: SessionFailure | null
  /** Hash of the final canonical event. */
  final_event_hash: string
  /** Stable format identifier. */
  format: string
  /** Total raw output bytes. */
  output_bytes: bigint
  /** Resource limit that caused host termination, when one was reached. */
  resource_limit?: ResourceLimitOutcome | null
  /** Session identifier. */
  session_id: string
  /** BLAKE3 of the exact immutable session manifest bytes. */
  session_manifest_hash: string
  /** Signal name when available. */
  signal_name?: string | null
  /** Sequence number of the required terminal `Exit` event. */
  terminal_event_seq: bigint
  /** Validation contract applied before publication. */
  validation_version: number
  /** Completion schema version. */
  version: number
}

/** Recorder failure retained when the child could not complete normally. */
export interface SessionFailure {
  /** Stable failure category. */
  code: string
  /** Human-readable failure context. */
  message: string
}

/** Initial PTY geometry. */
export interface SessionGeometry {
  /** Columns. */
  cols: number
  /** Total pixel height. */
  pixel_height: number
  /** Total pixel width. */
  pixel_width: number
  /** Rows. */
  rows: number
}

/** Where a session is in its life, observed without changing canonical evidence. This is the one session-phase vocabulary. Registry listings, CLI output, and live-control failures all report it rather than inventing their own. */
export type SessionPhase = "live" | "completed" | "incomplete" | "stale" | "missing_evidence" | "identity_mismatch" | "incompatible"

/** What [`Session::status`] observed. */
export type SessionStatus = SessionStatusLive | SessionStatusCompleted

/** The session has completed. */
export interface SessionStatusCompleted {
  /** Final completion record. */
  completion: SessionCompletion
  kind: "completed"
}

/** The live actor answered. */
export interface SessionStatusLive {
  /** Durable and observed canonical positions. */
  boundaries: ActorBoundaries
  /** Mapping between session-relative and system monotonic time. */
  clock_sample: ClockSample
  kind: "live"
  /** Child process identifier when available. */
  pid?: number | null
}

/** Deterministic reviewed representation of an already captured exact frame. */
export type SnapshotProfile = "text" | "spans" | "cells"

/** A non-empty half-open interval on the canonical monotonic timeline. */
export interface SourceRange {
  /** Exclusive source timestamp in nanoseconds. */
  end_monotonic_ns: bigint
  /** Inclusive source timestamp in nanoseconds. */
  start_monotonic_ns: bigint
}

/** Per-column content retained inside an appearance span. */
export interface SpanCell {
  /** Complete grapheme cluster. */
  grapheme: string
  /** Native Ghostty width role. */
  wide_kind: WideKind
}

/** Why an atomic stable-screen capture returned. */
export type StableCaptureQuality = "stable" | "child_completed" | "output_closed" | "deadline" | "unchanged" | "completed"

/** Atomic stable-screen outcome retaining the exact frame. The frame is not serialized; bindings attach it as a native frame object. */
export interface StableScreenCapture {
  /** First actor timestamp at which this exact state was observed. */
  first_observed_monotonic_ns: bigint
  /** Actor timestamp of the returned snapshot. */
  observed_monotonic_ns: bigint
  /** Why the capture returned; `Completed` when the session had already completed. */
  quality: StableCaptureQuality
  /** Zstandard-compressed canonical JSON of the captured live state; `None` once completed. */
  semantic_state_zstd?: Base64Bytes | null
  /** Requested unchanged interval. */
  stable_for_ns: bigint
  frame: FrameHandle
}

/** Delta from the previous state of the same replay. */
export interface StateDelta {
  /** New active SGR style when it changed. */
  active_style?: CellStyle | null
  /** New colors and palette when they changed. */
  colors?: ColorState | null
  /** New cursor when it changed. */
  cursor?: CursorState | null
  /** New `(cols, rows)` when geometry changed. */
  geometry?: [number, number] | null
  /** New terminal metadata when it changed. */
  metadata?: TerminalMetadata | null
  /** New terminal modes when they changed. */
  modes?: ModeState | null
  /** Authoritative wrap and cell deltas by row. */
  rows: Array<RowDiff>
}

/** Required relationship between the presented state and a supplied hash. */
export type StateHashComparison = "unchanged" | "changed"

/** Shared state selection vocabulary. */
export type StatePoint = StatePointLatestPresented | StatePointStateId | StatePointStateHash | StatePointEventSeq | StatePointMonotonicNs | StatePointMarker

/** Latest state at or before an event sequence. */
export interface StatePointEventSeq {
  /** Canonical event sequence. */
  event_seq: bigint
  selector: "event_seq"
}

/** Latest presentation-eligible state. */
export interface StatePointLatestPresented {
  selector: "latest_presented"
}

/** State at a uniquely named marker. */
export interface StatePointMarker {
  /** Unique marker label. */
  label: string
  selector: "marker"
}

/** Latest state at or before a session-monotonic timestamp. */
export interface StatePointMonotonicNs {
  /** Session-monotonic nanoseconds. */
  monotonic_ns: bigint
  selector: "monotonic_ns"
}

/** Latest state with this content hash. */
export interface StatePointStateHash {
  selector: "state_hash"
  /** BLAKE3 hash of the complete normalized semantic state. */
  state_hash: string
}

/** Stream position of the state: output bytes and applied resizes before it. */
export interface StatePointStateId {
  selector: "state_id"
  /** Stream position of the state: output bytes and applied resizes before it. */
  state_id: bigint
}

/** The semantic state at one stream position. */
export interface StateRecord {
  /** Stable delta from the preceding state of the same replay. */
  delta: StateDelta
  /** Zero-based byte offset inside an output event. */
  event_byte_offset?: bigint | null
  /** Canonical event responsible for this state; absent for an initial or range-baseline state. */
  event_seq?: bigint | null
  /** Ghostty's global dirty classification. */
  ghostty_dirty: DirtyState
  /** Rows Ghostty marked dirty. */
  ghostty_dirty_rows: Array<number>
  /** Monotonic event timestamp in nanoseconds. */
  monotonic_ns: bigint
  /** Whether this state was eligible for presentation. */
  presented: boolean
  /** BLAKE3 of the complete normalized semantic state. */
  state_hash: string
  /** Stream position: the output bytes and applied resizes that produced this state. */
  state_id: bigint
}

/** Outcome of a stop request with its request identity. */
export type StopReport = StopReportRequested | StopReportCompleted

/** Validated completion evidence, published before or after the request. */
export interface StopReportCompleted {
  /** Identity of the stop request. */
  request_id: string
  /** Final completion record. */
  completion: SessionCompletion
  kind: "completed"
}

/** The actor acknowledged the request; completion was not awaited. */
export interface StopReportRequested {
  /** Identity of the stop request. */
  request_id: string
  /** Canonical positions when the actor acknowledged the stop. */
  boundaries: ActorBoundaries
  kind: "requested"
}

/** Complete optional appearance constraints applied to every covered logical cell. */
export interface StyleFilter {
  /** Source background constraint. */
  background?: ColorSpec
  /** Blink constraint. */
  blink?: boolean
  /** Bold constraint. */
  bold?: boolean
  /** Faint constraint. */
  faint?: boolean
  /** Source foreground constraint. */
  foreground?: ColorSpec
  /** Inverse constraint. */
  inverse?: boolean
  /** Invisible constraint. */
  invisible?: boolean
  /** Italic constraint. */
  italic?: boolean
  /** Overline constraint. */
  overline?: boolean
  /** Resolved background constraint, including an explicit unresolved value. */
  resolved_background?: Rgb | null
  /** Resolved foreground constraint, including an explicit unresolved value. */
  resolved_foreground?: Rgb | null
  /** Selection constraint. */
  selected?: boolean
  /** Strikethrough constraint. */
  strikethrough?: boolean
  /** Native underline variant constraint. */
  underline?: number
  /** Source underline color constraint. */
  underline_color?: ColorSpec
  /** Authoritative terminal width-role constraint. */
  wide_kind?: WideKind
}

/** Caller-authored ASS resource copied and hash-manifested before rendering. */
export interface SubtitleTrack {
  /** ASS subtitle document. */
  ass: ExactPath
}

/** Result of observing a spawned successor. */
export type SuccessorReadiness = "live" | "completed" | "exited" | "timed_out" | "failed"

/** What a surviving path holds. */
export type SurvivingKind = "session" | "failed_visual" | "staging" | "sidecar" | "log"

/** A path verified to exist when the failure was reported. */
export interface SurvivingPathRecord {
  /** What the path holds. */
  kind: SurvivingKind
  /** Exact Linux path bytes. */
  path: Base64Bytes
}

/** A side effect the program asked the terminal for, as libghostty-vt reported it. */
export type TerminalEffect = TerminalEffectBell | TerminalEffectTitleChanged | TerminalEffectWorkingDirectoryChanged | TerminalEffectClipboardWrite | TerminalEffectDesktopNotification | TerminalEffectProgressReport | TerminalEffectFullReset | TerminalEffectWindowResizeRequest | TerminalEffectPromptStart | TerminalEffectInputStart | TerminalEffectCommandStart | TerminalEffectCommandEnd | TerminalEffectUnknownSequence | TerminalEffectProcessingError

/** BEL outside any control string. */
export interface TerminalEffectBell {
  type: "bell"
}

/** OSC 52, OSC 1337, or OSC 5522 asked to write the clipboard. */
export interface TerminalEffectClipboardWrite {
  /** Representations of the written value; empty clears the clipboard. */
  representations: Array<ClipboardRepresentation>
  /** Destination clipboard. */
  target: ClipboardTarget
  type: "clipboard_write"
}

/** OSC 133 marked the command finishing. */
export interface TerminalEffectCommandEnd {
  /** Error description when the shell sent one. */
  error?: Base64Bytes | null
  /** Exit code when the shell reported one. */
  exit_code?: number | null
  type: "command_end"
}

/** OSC 133 marked the submitted command starting to run. */
export interface TerminalEffectCommandStart {
  /** Decoded command line when the shell sent one. */
  command?: Base64Bytes | null
  type: "command_start"
}

/** OSC 9 or OSC 777 asked for a desktop notification. */
export interface TerminalEffectDesktopNotification {
  /** Digest of the notification body. */
  body: PayloadDigest
  /** Digest of the notification title. */
  title: PayloadDigest
  type: "desktop_notification"
}

/** RIS reset the terminal. */
export interface TerminalEffectFullReset {
  type: "full_reset"
}

/** OSC 133 marked the end of the prompt and the start of command input. */
export interface TerminalEffectInputStart {
  type: "input_start"
}

/** libghostty-vt hit an error that may have prevented a semantic update. */
export interface TerminalEffectProcessingError {
  type: "processing_error"
}

/** OSC 9;4 reported progress. */
export interface TerminalEffectProgressReport {
  /** Percentage from 0 through 100 when the program sent one. */
  percent?: number | null
  /** Reported state. */
  state: ProgressState
  type: "progress_report"
}

/** OSC 133 started drawing a prompt. */
export interface TerminalEffectPromptStart {
  /** Which prompt started. */
  prompt: PromptKind
  type: "prompt_start"
}

/** OSC 0 or OSC 2 set the window title. */
export interface TerminalEffectTitleChanged {
  /** New title bytes. */
  title: Base64Bytes
  type: "title_changed"
}

/** The program sent an APC or OSC string that the semantic engine ignored. */
export interface TerminalEffectUnknownSequence {
  /** Leading command name or number: at most 16 ASCII alphanumeric, `-`, `_`, or `.` bytes. */
  identifier: Base64Bytes
  /** Control string family. */
  kind: UnknownSequenceKind
  type: "unknown_sequence"
}

/** CSI 8 t asked to resize the window; the terminal itself is not resized. */
export interface TerminalEffectWindowResizeRequest {
  /** Requested columns, absent to keep the current width. */
  columns?: number | null
  /** Requested rows, absent to keep the current height. */
  rows?: number | null
  type: "window_resize_request"
}

/** OSC 7, OSC 9, or OSC 1337 reported the working directory. */
export interface TerminalEffectWorkingDirectoryChanged {
  /** Reported location, usually a `file://` URI; empty clears it. */
  location: Base64Bytes
  type: "working_directory_changed"
}

/** Geometry reported by a live terminal through XTWINOPS queries. */
export interface TerminalGeometryReport {
  /** Height of one rendered cell in pixels. */
  cell_height: number
  /** Width of one rendered cell in pixels. */
  cell_width: number
  /** Terminal columns. */
  cols: number
  /** Terminal rows. */
  rows: number
  /** Height of the complete text area in pixels. */
  text_height: number
  /** Width of the complete text area in pixels. */
  text_width: number
}

/** Terminal metadata controlled by OSC sequences. */
export interface TerminalMetadata {
  /** Raw title bytes. */
  title: Base64Bytes
  /** Raw working-directory bytes. */
  working_directory: Base64Bytes
}

/** One ANSI or DEC mode value. */
export interface TerminalMode {
  /** `true` for ANSI mode, `false` for DEC-private mode. */
  ansi: boolean
  /** Current set/reset value. */
  enabled: boolean
  /** Numeric mode identifier. */
  number: number
}

/** Classification recorded when an owned test is finalized. */
export type TestOutcome = "passed" | "assertion_failed" | "error" | "panic" | "timed_out"

/** One text annotation with an exact scope, position, and visible style. */
export interface TextAnnotation {
  /** Exact anchored placement. */
  position?: AnnotationPosition
  /** Output/source selection scope. */
  scope: AnnotationScope
  /** Visible text/background style. */
  style?: AnnotationStyle
  /** Dynamic template expanded per output frame. */
  template?: string | null
  /** Literal text; mutually exclusive with `template`. */
  text?: string | null
  /** Stable overlay ordering. */
  z_order?: number
}

/** One ordered, source-bound timeline item. */
export interface TimelineEntry {
  /** Responsible canonical event, absent for the initial semantic state. */
  event_seq?: bigint | null
  /** Timeline item payload. */
  item: TimelineItem
  /** Canonical session offset. */
  monotonic_ns: bigint
}

/** Concise canonical or semantic item in a diagnostic timeline. */
export type TimelineItem = TimelineItemSemanticState | TimelineItemTerminalEffect | TimelineItemCanonicalEvent | TimelineItemVtToken

/** A non-output canonical event retained verbatim. */
export interface TimelineItemCanonicalEvent {
  /** Exact typed canonical event. */
  event: Event
  kind: "canonical_event"
}

/** A Ghostty semantic boundary state. */
export interface TimelineItemSemanticState {
  /** Number of complete semantic cell values changed. */
  changed_cells: bigint
  /** Rows whose semantic cells changed. */
  changed_rows: Array<number>
  /** Whether cursor state changed. */
  cursor_changed: boolean
  /** Byte offset within the responsible output event. */
  event_byte_offset?: bigint | null
  /** Ghostty's dirty classification. */
  ghostty_dirty: DirtyState
  kind: "semantic_state"
  /** Whether terminal metadata changed. */
  metadata_changed: boolean
  /** Whether terminal modes changed. */
  modes_changed: boolean
  /** Whether the state was eligible for presentation. */
  presented: boolean
  /** Complete normalized semantic state hash. */
  state_hash: string
  /** Stream position of the state. */
  state_id: bigint
}

/** A side effect libghostty-vt reported while replaying output. */
export interface TimelineItemTerminalEffect {
  /** Reported effect. */
  effect: TerminalEffect
  /** Offset of the completing byte within the responsible output event. */
  event_byte_offset: bigint
  kind: "terminal_effect"
}

/** One lossless explanatory VT frame. */
export interface TimelineItemVtToken {
  kind: "vt_token"
  /** Exact token and canonical source segments. */
  token: VtToken
}

/** One bounded page of a concise diagnostic timeline. */
export interface TimelinePage {
  /** Entries selected by the requested offset and limit. */
  entries: Array<TimelineEntry>
  /** Offset for the next page, absent when this page reaches closure. */
  next_offset?: bigint | null
}

/** One track or coverage-gap catalog page with its manifest and summary. */
export type TmuxCatalog = TmuxCatalogTracks | TmuxCatalogGaps

/** Coverage intervals. */
export interface TmuxCatalogGaps {
  /** Requested page of gap entries. */
  entries: Array<TmuxGapCatalogEntry>
  kind: "gaps"
  /** Aggregate recording manifest. */
  manifest: TmuxRecordingManifest
  /** Catalog totals. */
  summary: TmuxCatalogSummary
}

/** Counts committed with the lifecycle journal's indexed inventory. */
export interface TmuxCatalogSummary {
  /** All coverage intervals, including resolved intervals. */
  coverage_gaps: bigint
  /** Recording incarnations retained in complete history. */
  tracks: bigint
  /** Linked incarnations without an observed canonical completion. */
  unfinished_tracks: bigint
}

/** Track incarnations. */
export interface TmuxCatalogTracks {
  /** Requested page of track entries. */
  entries: Array<TmuxTrackCatalogEntry>
  kind: "tracks"
  /** Aggregate recording manifest. */
  manifest: TmuxRecordingManifest
  /** Catalog totals. */
  summary: TmuxCatalogSummary
}

/** Link from an aggregate recording to its outer tmux client recording. */
export interface TmuxCompositeTrack {
  /** Registry alias for the outer-client track. */
  alias: string
  /** Canonical session once the composite client started. */
  link?: TmuxSessionLink | null
}

/** Existing uncovered-pane policies. */
export type TmuxCoverage = "warn" | "strict"

/** One uncovered pane or explicit observation limitation. */
export interface TmuxCoverageGap {
  /** First observation wall-clock time. */
  first_observed_unix_ns: bigint
  /** Gap category. */
  kind: TmuxGapKind
  /** Most recent observation wall-clock time. */
  last_observed_unix_ns: bigint
  /** Related pane ID, or none for an aggregate limitation. */
  pane_id?: string | null
  /** Wall-clock time when the gap was first observed to be resolved. */
  resolved_unix_ns?: bigint | null
}

/** One sequenced, timestamped lifecycle event as committed to the journal. */
export type TmuxEventRecord = TmuxEventRecordRecordingStarted | TmuxEventRecordCompositeTrackLinked | TmuxEventRecordRecordingStopping | TmuxEventRecordRecordingCompleted | TmuxEventRecordServerIdentityBound | TmuxEventRecordObserverStarted | TmuxEventRecordSessionCreated | TmuxEventRecordSessionDestroyed | TmuxEventRecordWindowCreated | TmuxEventRecordWindowDestroyed | TmuxEventRecordPaneCreated | TmuxEventRecordPaneDestroyed | TmuxEventRecordPaneMoved | TmuxEventRecordLayoutChanged | TmuxEventRecordActiveWindowChanged | TmuxEventRecordActivePaneChanged | TmuxEventRecordZoomChanged | TmuxEventRecordPaneModeChanged | TmuxEventRecordPaneTrackLinked | TmuxEventRecordPaneTrackCompleted | TmuxEventRecordCoverageGapOpened | TmuxEventRecordCoverageGapResolved

/** A pane became active or inactive in its window. */
export interface TmuxEventRecordActivePaneChanged {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  /** Whether the pane is now active. */
  active: boolean
  kind: "active_pane_changed"
  /** Pane identifiers. */
  pane: TmuxPaneAddress
}

/** A pane's window became active or inactive. */
export interface TmuxEventRecordActiveWindowChanged {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  /** Whether the window is now active. */
  active: boolean
  kind: "active_window_changed"
  /** Pane identifiers. */
  pane: TmuxPaneAddress
}

/** The composite client track started. */
export interface TmuxEventRecordCompositeTrackLinked {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "composite_track_linked"
  /** Composite session identity and path. */
  link: TmuxSessionLink
}

/** A coverage interval opened. */
export interface TmuxEventRecordCoverageGapOpened {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  /** Opened interval. */
  gap: TmuxCoverageGap
  kind: "coverage_gap_opened"
}

/** A coverage interval resolved. */
export interface TmuxEventRecordCoverageGapResolved {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  /** Final interval state. */
  gap: TmuxCoverageGap
  kind: "coverage_gap_resolved"
}

/** A pane's window layout changed. */
export interface TmuxEventRecordLayoutChanged {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "layout_changed"
  /** New layout. */
  layout: string
  /** Pane identifiers. */
  pane: TmuxPaneAddress
}

/** A supervisor process started observing the server. */
export interface TmuxEventRecordObserverStarted {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "observer_started"
  /** Supervisor process ID. */
  pid: number
}

/** A pane appeared. */
export interface TmuxEventRecordPaneCreated {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "pane_created"
  /** Pane identifiers. */
  pane: TmuxPaneAddress
}

/** A pane disappeared. */
export interface TmuxEventRecordPaneDestroyed {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "pane_destroyed"
  /** Last observed pane identifiers. */
  pane: TmuxPaneAddress
}

/** A pane entered, changed, or left a tmux mode such as copy mode. */
export interface TmuxEventRecordPaneModeChanged {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "pane_mode_changed"
  /** Active mode name, or none when no mode is active. */
  mode?: string | null
  /** Pane identifiers. */
  pane: TmuxPaneAddress
}

/** A pane moved into another window. */
export interface TmuxEventRecordPaneMoved {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "pane_moved"
  /** Pane identifiers after the move. */
  pane: TmuxPaneAddress
}

/** A linked pane recording published canonical completion. */
export interface TmuxEventRecordPaneTrackCompleted {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "pane_track_completed"
  /** Stable pane ID. */
  pane_id: string
  /** Completed recording identity. */
  session_id: string
}

/** A pane recording incarnation was linked. */
export interface TmuxEventRecordPaneTrackLinked {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "pane_track_linked"
  /** Linked incarnation. */
  track: TmuxPaneTrack
}

/** Every track completed; no further events follow. */
export interface TmuxEventRecordRecordingCompleted {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "recording_completed"
}

/** The aggregate identity was created. */
export interface TmuxEventRecordRecordingStarted {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  /** Committed coverage policy. */
  coverage: TmuxRecordingCoverage
  kind: "recording_started"
}

/** A stop request was committed. */
export interface TmuxEventRecordRecordingStopping {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "recording_stopping"
}

/** The first reachable server identity was bound. */
export interface TmuxEventRecordServerIdentityBound {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  /** Bound identity. */
  identity: TmuxServerIdentity
  kind: "server_identity_bound"
}

/** A tmux session appeared. */
export interface TmuxEventRecordSessionCreated {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "session_created"
  /** Stable tmux session ID. */
  session_id: string
}

/** A tmux session disappeared. */
export interface TmuxEventRecordSessionDestroyed {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "session_destroyed"
  /** Stable tmux session ID. */
  session_id: string
}

/** A tmux window appeared. */
export interface TmuxEventRecordWindowCreated {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "window_created"
  /** Stable tmux window ID. */
  window_id: string
}

/** A tmux window disappeared. */
export interface TmuxEventRecordWindowDestroyed {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "window_destroyed"
  /** Stable tmux window ID. */
  window_id: string
}

/** A pane's window was zoomed or unzoomed. */
export interface TmuxEventRecordZoomChanged {
  /** Observation wall-clock time. */
  observed_unix_ns: bigint
  /** Monotonic sequence within this aggregate, starting at one. */
  sequence: bigint
  kind: "zoom_changed"
  /** Pane identifiers. */
  pane: TmuxPaneAddress
  /** Whether the window is now zoomed. */
  zoomed: boolean
}

/** One aggregate history page with the manifest it belongs to. */
export interface TmuxEvents {
  /** Aggregate recording manifest. */
  manifest: TmuxRecordingManifest
  /** Requested history page. */
  page: EventPage
}

/** Actual tmux progress receipts, independent of the actor mutation/retry protocol. */
export interface TmuxFailureFacts {
  /** Management entrypoint, not an actor request identifier. */
  action: string
  /** Tracks whose stop operation returned successfully (not a rollback guarantee). */
  completed_track_stops: Array<Base64Bytes>
  /** Tracks whose completion wait returned successfully. */
  completed_track_waits: Array<Base64Bytes>
  /** Whether integration disable was attempted, not whether it succeeded. */
  integration_disable_attempted: boolean
  /** Exact aggregate manifest path, when known. */
  manifest?: Base64Bytes | null
  /** Pane shutdown commands attempted; command success is not asserted. */
  pane_shutdown_attempted: Array<string>
  /** Last known phase. */
  phase: TmuxFailurePhase
  /** Whether `StopRequested` is proven committed; None means no such receipt is available. */
  stop_requested?: boolean | null
}

/** Last known tmux management phase; none of these are input-atom counts. */
export type TmuxFailurePhase = "inspect_state" | "stop_requested" | "composite_stop_completed" | "waiting_tracks" | "settling"

/** One historical coverage interval from the lifecycle catalog. */
export interface TmuxGapCatalogEntry {
  /** Latest committed state of the interval. */
  gap: TmuxCoverageGap
  /** Lifecycle sequence that opened this interval. */
  sequence: bigint
}

/** Category of an uncovered pane or observation limitation. */
export type TmuxGapKind = "polling_interval" | "client_overlay_index" | "observer_start" | "uncovered_pane"

/** Existing Ghostty pane capture policies. */
export type TmuxGhosttyProfile = "auto" | "required" | "off"

/** Committed prefix of the append-only lifecycle journal beside the snapshot. */
export interface TmuxJournalBoundary {
  /** Complete newline-framed bytes committed by this snapshot. */
  bytes: bigint
  /** Hash of the last journal record. */
  hash: string
  /** Last committed lifecycle sequence. */
  sequence: bigint
  /** Journal record shape version. */
  version: number
}

/** Stable tmux object identifiers of one pane at observation time. */
export interface TmuxPaneAddress {
  /** Stable tmux pane ID. */
  pane_id: string
  /** Stable tmux session ID. */
  session_id: string
  /** Stable tmux window ID. */
  window_id: string
}

/** Latest supervisor observation for one tmux pane. */
export interface TmuxPaneSnapshot {
  /** Whether the pane is in a tmux mode such as copy mode. */
  copy_mode: boolean
  /** Current tmux window layout. */
  layout: string
  /** Whether this pane is active in its window. */
  pane_active: boolean
  /** Stable tmux pane ID. */
  pane_id: string
  /** Mode name reported by tmux. */
  pane_mode: string
  /** Canonical pane-specific Termtrace session path when covered. */
  recording_session?: Base64Bytes | null
  /** Stable tmux session ID. */
  session_id: string
  /** Human-readable tmux session name. */
  session_name: string
  /** Whether this pane belongs to the active window. */
  window_active: boolean
  /** Stable tmux window ID. */
  window_id: string
  /** Current tmux window index. */
  window_index: number
  /** Whether the pane's window is zoomed. */
  zoomed: boolean
}

/** One pane in a status report. */
export interface TmuxPaneStatus {
  /** Current foreground command. */
  current_command: Base64Bytes
  /** Stable pane ID. */
  pane_id: string
  /** Pane root process. */
  pane_pid?: number | null
  /** Run by a pane wrapper. */
  recorded: boolean
  /** Published recording session. */
  recording_session?: Base64Bytes | null
  /** In scope. */
  selected: boolean
  /** Stable session ID. */
  session_id: string
  /** Stable window ID. */
  window_id: string
}

/** One immutable pane recording incarnation linked to an aggregate. */
export interface TmuxPaneTrack {
  /** Time this incarnation was first linked to the aggregate. */
  linked_unix_ns: bigint
  /** Stable pane ID; a pane may have arbitrarily many recording incarnations. */
  pane_id: string
  /** Exact canonical session path. */
  session: Base64Bytes
  /** Immutable canonical recording identity. */
  session_id: string
}

/** Aggregate start settings. Exact bytes identify command arguments, targets, and paths. */
export interface TmuxRecordConfiguration {
  /** Fallback cell height, matching CLI defaults. */
  cell_height?: number
  /** Fallback cell width, matching CLI defaults. */
  cell_width?: number
  /** Full recorded tmux client command; its executable must match the selected namespace client. */
  command?: Array<Base64Bytes>
  /** Existing integration's uncovered-pane policy. */
  coverage?: TmuxCoverage
  /** Record only explicitly launched panes. */
  explicit_only?: boolean
  /** Explicit Ghostty configuration path, if needed. */
  ghostty_config?: Base64Bytes | null
  /** Existing integration's Ghostty capture policy. */
  ghostty_profile?: TmuxGhosttyProfile
  /** Use the existing patched pane-launch hook. */
  launch_hook?: boolean
  /** Unique aggregate name, validated by the existing tmux store. */
  name?: string
  /** Start a new isolated server; false enables an existing selected server. */
  new_server?: boolean
  /** Initial stable pane scope targets. */
  panes?: Array<Base64Bytes>
  /** Opt-in completed-pane retention budget; absent means no budget-based deletion. */
  retain_bytes?: bigint | null
  /** Opt-in completed-pane retention age; absent means no age-based deletion. */
  retain_days?: bigint | null
  /** Initial session scope targets, resolved by the existing tmux service. */
  sessions?: Array<Base64Bytes>
  /** Optional terminal identity override. */
  term?: string | null
  /** Initial window scope targets. */
  windows?: Array<Base64Bytes>
}

/** Coverage policy committed by an aggregate tmux recording. */
export type TmuxRecordingCoverage = "warn" | "strict"

/** Aggregate recording lifecycle; see [`crate::lifecycle`] for its legal transitions. */
export type TmuxRecordingLifecycle = "starting" | "live" | "stopping" | "completed"

/** Aggregate identity and current state; complete history lives in the lifecycle journal. The snapshot holds no history: events, resolved gaps, and every track incarnation are read through the journal-backed catalog. */
export interface TmuxRecordingManifest {
  /** Complete historical counts indexed from the journal. */
  catalog: TmuxCatalogSummary
  /** Link to the composite outer-client recording. */
  composite_track: TmuxCompositeTrack
  /** Coverage enforcement policy. */
  coverage: TmuxRecordingCoverage
  /** Unresolved coverage gaps; resolved intervals live in the journal and catalog. */
  coverage_gaps: Array<TmuxCoverageGap>
  /** Creation wall-clock time. */
  created_unix_ns: bigint
  /** Stable format identifier. */
  format: string
  /** Aggregate lifecycle. */
  lifecycle: TmuxRecordingLifecycle
  /** Committed lifecycle journal prefix. */
  lifecycle_journal: TmuxJournalBoundary
  /** Friendly aggregate recording name. */
  name: string
  /** Present pane IDs linked to their current pane-specific session path. */
  pane_tracks: Record<string, Base64Bytes>
  /** Latest per-pane observations. */
  panes: Array<TmuxPaneSnapshot>
  /** Immutable aggregate recording identity. */
  recording_id: string
  /** Immutable tmux server identity once observed. */
  server?: TmuxServerIdentity | null
  /** Configured tmux server socket. */
  socket: TmuxSocket
  /** Exact tmux executable path bytes. */
  tmux_executable: Base64Bytes
  /** Last state change wall-clock time. */
  updated_unix_ns: bigint
  /** Manifest schema version. */
  version: number
}

/** Aggregate admission result with an exact binding to its persisted manifest. */
export interface TmuxRecordingStart {
  /** Exact manifest path bytes; authoritative for subsequent track/bundle operations. */
  manifest: Base64Bytes
  /** Human-readable manifest path, explicitly nonauthoritative. */
  manifest_display: string
  /** Existing aggregate recording contract, including real coverage and lifecycle. */
  recording: TmuxRecordingManifest
}

/** Recording scope mode. */
export type TmuxScopeMode = "all" | "explicit" | "selected"

/** Versioned recording scope stored in a global tmux option. */
export interface TmuxScopePolicy {
  /** Scope mode. */
  mode: TmuxScopeMode
  /** Selected stable pane IDs. */
  pane_ids: Array<string>
  /** Selected stable session IDs. */
  session_ids: Array<string>
  /** Selected session names. */
  session_names: Array<Base64Bytes>
  /** Policy shape version. */
  version: number
  /** Selected stable window IDs. */
  window_ids: Array<string>
  /** Selected window names. */
  window_names: Array<Base64Bytes>
}

/** Immutable tmux server identity bound by the first successful observation. */
export interface TmuxServerIdentity {
  /** tmux server PID. */
  pid: number
  /** Device containing the server socket. */
  socket_device: bigint
  /** Inode of the server socket. */
  socket_inode: bigint
  /** Server socket path bytes. */
  socket_path: Base64Bytes
}

/** Immutable identity and exact path of one canonical Termtrace session. */
export interface TmuxSessionLink {
  /** Exact canonical session path bytes. */
  session: Base64Bytes
  /** Canonical Termtrace session identity. */
  session_id: string
}

/** How a tmux client reaches its server. */
export type TmuxSocket = "default" | TmuxSocketName | TmuxSocketPath

/** A socket name, equivalent to `tmux -L`. */
export interface TmuxSocketName {
  name: Base64Bytes
}

/** A socket path, equivalent to `tmux -S`. */
export interface TmuxSocketPath {
  path: Base64Bytes
}

/** Integration and coverage status of one tmux server. */
export interface TmuxStatusReport {
  /** Configured Termtrace executable. */
  configured_executable?: Base64Bytes | null
  /** The launch option still runs the pane wrapper. */
  default_command_matches: boolean
  /** The current integration version is enabled. */
  enabled: boolean
  /** Panes not selected. */
  excluded_panes: bigint
  /** Integration contract version. */
  integration_version: number
  /** Live observer processes. */
  observers?: Observers | null
  /** Managed state root. */
  output_root?: Base64Bytes | null
  /** Live panes. */
  pane_count: bigint
  /** Every live pane. */
  panes: Array<TmuxPaneStatus>
  /** Panes run by a wrapper. */
  recorded_panes: bigint
  /** Recording scope. */
  scope?: TmuxScopePolicy | null
  /** Selected panes. */
  selected_panes: bigint
  /** Coverage is strict. */
  strict: boolean
  /** Selected panes not run by a wrapper. */
  unrecorded_panes: bigint
}

/** Bounded manifest for an aggregate export with a streamed track inventory. */
export interface TmuxStreamingBundleManifest {
  /** Exact source aggregate manifest path bytes. */
  aggregate_manifest: Base64Bytes
  /** BLAKE3 of the caller-owned aggregate snapshot. */
  aggregate_manifest_blake3: string
  /** Stable format identifier. */
  format: string
  /** Immutable aggregate recording identity. */
  recording_id: string
  /** Friendly recording name. */
  recording_name: string
  /** Number of nested forensic bundles, including the composite. */
  track_count: bigint
  /** BLAKE3 of tracks.jsonl; each row is a `TmuxBundleTrack`. */
  track_inventory_blake3: string
  /** Exact byte count of tracks.jsonl. */
  track_inventory_bytes: bigint
  /** Manifest schema version. */
  version: number
}

/** One bounded-page result from the complete recording inventory. */
export interface TmuxTrackCatalogEntry {
  /** Whether canonical completion was observed. */
  completed: boolean
  /** Lifecycle sequence that first linked this incarnation. */
  sequence: bigint
  /** Immutable pane recording identity and path. */
  track: TmuxPaneTrack
}

/** Rendering truth represented by an observation. */
export type TruthLayer = "semantic" | "native"

/** Family of a control string the semantic engine does not implement. */
export type UnknownSequenceKind = "apc" | "osc"

/** Before/after values for one changed semantic property. */
export interface ValueChange {
  /** Value at the later endpoint. */
  after: CursorState
  /** Value at the earlier endpoint. */
  before: CursorState
}

/** Before/after values for one changed semantic property. */
export interface ValueChange2 {
  /** Value at the later endpoint. */
  after: [number, number]
  /** Value at the earlier endpoint. */
  before: [number, number]
}

/** Before/after values for one changed semantic property. */
export interface ValueChange3 {
  /** Value at the later endpoint. */
  after: ModeState
  /** Value at the earlier endpoint. */
  before: ModeState
}

/** Before/after values for one changed semantic property. */
export interface ValueChange4 {
  /** Value at the later endpoint. */
  after: ColorState
  /** Value at the earlier endpoint. */
  before: ColorState
}

/** Before/after values for one changed semantic property. */
export interface ValueChange5 {
  /** Value at the later endpoint. */
  after: CellStyle
  /** Value at the earlier endpoint. */
  before: CellStyle
}

/** Before/after values for one changed semantic property. */
export interface ValueChange6 {
  /** Value at the later endpoint. */
  after: TerminalMetadata
  /** Value at the earlier endpoint. */
  before: TerminalMetadata
}

/** Before/after values for one changed semantic property. */
export interface ValueChange7 {
  /** Value at the later endpoint. */
  after: boolean
  /** Value at the earlier endpoint. */
  before: boolean
}

/** Before/after values for one changed semantic property. */
export interface ValueChange8 {
  /** Value at the later endpoint. */
  after?: boolean | null
  /** Value at the earlier endpoint. */
  before?: boolean | null
}

/** One stable verification diagnostic. */
export interface VerificationFinding {
  /** Stable machine-readable finding code. */
  code: string
  /** Human-readable context that is not intended for programmatic matching. */
  message: string
}

/** Requested depth for an offline evidence verification. */
export type VerificationLevel = "structure" | "content" | "durability_record" | "signature"

/** Machine-readable result from offline session verification. */
export interface VerificationReport {
  /** Whether artifact authenticity was established from a trusted checkpoint. */
  artifact_authenticity_established: boolean
  /** Number of locally cataloged artifact roots verified at content level. */
  artifact_roots_verified: bigint
  /** Final canonical event hash, when readable. */
  final_event_hash?: string | null
  /** Stable findings; empty when verification succeeds. */
  findings: Array<VerificationFinding>
  /** Stable report format identifier. */
  format: string
  /** Requested verification depth. */
  level: VerificationLevel
  /** Journal-only diagnostic when acceleration is invalid. */
  mutation_journal_error?: string | null
  /** Number of mutation outcomes verified against canonical ranges. */
  mutation_journal_records_verified: bigint
  /** State of the disposable mutation deduplication acceleration journal. */
  mutation_journal_status: MutationJournalStatus
  /** Page coordinates when CLI output intentionally contains a bounded subset. */
  output_page?: OutputPage | null
  /** Canonical session directory checked by the verifier. */
  session: ExactPath
  /** Whether canonical session authenticity was established from a trusted receipt. */
  session_authenticity_established: boolean
  /** Immutable session identifier, when readable. */
  session_id?: string | null
  /** Exact session-manifest hash, when readable. */
  session_manifest_hash?: string | null
  /** Whether the detached signature matches the exact receipt payload. */
  signature_cryptographic_valid: boolean
  /** Whether a detached evidence receipt was present. */
  signature_receipt_present: boolean
  /** Signer identity asserted by the detached receipt. */
  signature_signer_key_id?: string | null
  /** Whether the configured trust policy accepts the signer identity/key. */
  signature_signer_trusted: boolean
  /** Whether every invariant at the requested level passed. */
  valid: boolean
  /** Report schema version. */
  version: number
}

/** Build and component versions of this Termtrace library. */
export interface Versions {
  /** Control protocol version. */
  control_protocol: string
  /** Pinned Ghostty VT revision. */
  ghostty_revision: string
  /** Target CPU architecture. */
  target_arch: string
  /** Target operating system. */
  target_os: string
  /** Termtrace package version. */
  termtrace: string
}

/** SDK options for a video of an explicit edit. */
export interface VideoOptions {
  /** Exact output frame rate; a native capture must match. Defaults to 30. */
  fps?: number | null
  /** GIF loop policy; GIF render only. */
  gif_loop?: GifLoop | null
  /** Let an MP4 render replace an existing file; render only. */
  overwrite?: boolean
  /** Storyboard and review-video settings; preview only. */
  preview?: PreviewOptions | null
  /** Pixel source; native never falls back to semantic. */
  renderer: VideoRenderer
  /** Semantic rasterization quality; semantic renderer only. */
  semantic_raster_profile?: SemanticRasterProfile | null
}

/** A rendered preview and its plan. */
export interface VideoPreview {
  /** Contact-sheet PNG. */
  contact_sheet: ExactPath
  /** No-replace preview directory. */
  directory: ExactPath
  /** Rendered plan. */
  plan: PresentationPlan
  /** Low-resolution H.264 MP4. */
  video: ExactPath
}

/** A published video, its plan, and its manifested sidecar. */
export interface VideoRender {
  /** Shared artifact envelope of the sidecar; present for GIF and `WebM`. */
  artifact?: MediaArtifactResult | null
  /** Published MP4, GIF, or `WebM`. */
  output: ExactPath
  /** Rendered plan. */
  plan: PresentationPlan
  /** Manifested render directory kept next to the output. */
  sidecar: ExactPath
}

/** Effective user-facing video facade settings. */
export interface VideoRenderConfig {
  /** Effective cursor presentation style. */
  cursor_style: string
  /** Optional exclusive marker. */
  end_marker?: string | null
  /** Effective output frame rate. */
  fps: number
  /** Generated closing hold. */
  hold_end_ms: bigint
  /** Generated opening hold. */
  hold_start_ms: bigint
  /** Idle gap threshold used by trimming. */
  idle_threshold_ms?: bigint | null
  /** Named delivery preset. */
  preset: string
  /** Requested renderer without fallback. */
  renderer: string
  /** Exact requested output resolution. */
  resolution?: Array<number> | null
  /** Requested integer scale factor. */
  scale?: number | null
  /** Optional inclusive marker. */
  start_marker?: string | null
  /** Whether long gaps without child output were removed. */
  trim_idle: boolean
}

/** Where a video's pixels come from. */
export type VideoRenderer = "native" | "semantic" | "native_replay" | "original_execution"

/** Immutable visual configuration selected for one launch. The native section configures Ghostty for native pixels. The semantic section selects the bundled semantic renderer and never changes native configuration. */
export interface VisualProfile {
  /** Native Ghostty configuration; selecting it requires Ghostty and the named fonts. */
  native?: NativeVisualProfile | null
  /** Semantic renderer selection. */
  semantic?: SemanticVisualProfile | null
  /** Contract version; exactly 1. */
  version: number
}

/** Stable visual-profile failure categories shared by every SDK. */
export type VisualProfileFailure = "invalid_visual_profile" | "native_profile_unavailable" | "ghostty_config_invalid" | "font_unavailable" | "viewport_mismatch" | "native_profile_failed"

/** Result of validating a visual profile before launch. The report holds settings and resource identities only, never local paths or timestamps. It is advisory: a later launch resolves the profile again. */
export interface VisualProfileReport {
  /** What a launch with this profile guarantees. */
  guarantees: Array<string>
  /** What it does not guarantee. */
  limitations: Array<string>
  /** Native resolution facts, when the native section is selected. */
  native?: NativeProfileReport | null
  /** Normalized profile. */
  profile: VisualProfile
  /** BLAKE3 identity of `profile`. */
  profile_hash: string
  /** Bundled semantic resources, when the semantic section is selected. */
  semantic?: SemanticImageResource | null
  /** Report version; exactly 1. */
  version: number
}

/** Half-open byte range within one canonical output event. */
export interface VtSourceSegment {
  /** Exclusive byte offset within the event payload. */
  end_byte_offset: bigint
  /** Canonical output event sequence. */
  event_seq: bigint
  /** Session-monotonic event timestamp. */
  monotonic_ns: bigint
  /** Inclusive byte offset within the event payload. */
  start_byte_offset: bigint
}

/** One lossless VT frame with exact canonical source ranges. */
export interface VtToken {
  /** Stable neutral description for common controls. */
  description: string
  /** Framed sequence family. */
  kind: VtTokenKind
  /** Exact bytes, including introducer and terminator. */
  raw: Base64Bytes
  /** Ordered exact source segments; concatenating them reproduces `raw`. */
  source: Array<VtSourceSegment>
  /** Timing resolution; current recordings timestamp complete PTY reads. */
  timing_resolution: string
  /** Dense token identifier. */
  token_id: bigint
}

/** Stable VT token family. This is framing, not terminal interpretation. */
export type VtTokenKind = "printable" | "c0" | "c1" | "esc" | "csi" | "osc" | "dcs" | "apc" | "unknown" | "incomplete"

/** Successful raw-output wait evidence. */
export interface WaitOutcome {
  /** Event containing the end of the first observed match. */
  event_seq: bigint
  /** Monotonic timestamp of the containing event. */
  monotonic_ns: bigint
  /** End offset in the concatenated output stream. */
  output_end_offset: bigint
}

/** Authoritative width role reported by Ghostty for one terminal cell. */
export type WideKind = "narrow" | "wide" | "spacer_tail" | "spacer_head"

/** Solver-selected Ghostty window padding in points; zero leaves the profile's own padding in effect. */
export interface WindowPadding {
  /** Horizontal padding on each side. */
  x: number
  /** Vertical padding on each side. */
  y: number
}

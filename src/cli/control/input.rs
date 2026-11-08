use super::*;
use crate::cli::parsers::parse_stability_duration;

/// Parse one `send` input atom: a key chord, `text:`, `paste:`, `raw:`, `raw-file:`, or one
/// printable character.
pub(super) fn parse_atom(atom: &str) -> Result<InputAtom> {
    if let Some(duration) = atom.strip_prefix("sleep:") {
        return Ok(InputAtom::Sleep(
            parse_stability_duration(duration).map_err(|error| miette::miette!("{error}"))?,
        ));
    }

    if let Some(chord) = KeyChord::parse(atom).into_diagnostic()? {
        return Ok(InputAtom::Key(chord));
    }

    if let Some(paste) = atom.strip_prefix("paste:") {
        return Ok(InputAtom::Paste(paste.to_owned()));
    }

    if let Some(text) = atom.strip_prefix("text:") {
        return Ok(InputAtom::Text(text.to_owned()));
    }

    if let Some(raw) = atom.strip_prefix("raw:") {
        return Ok(InputAtom::Bytes(raw.as_bytes().to_vec()));
    }

    if let Some(path) = atom.strip_prefix("raw-file:") {
        return Ok(InputAtom::Bytes(fs_err::read(path).into_diagnostic()?));
    }

    if atom.chars().count() == 1 && atom.chars().all(|character| return !character.is_control()) {
        return Ok(InputAtom::Text(atom.to_owned()));
    }
    return Err(miette::miette!(
        "unknown input atom; run `termtrace send --help` or use text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH"
    ));
}

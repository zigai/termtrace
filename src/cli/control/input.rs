use super::*;

/// Parse one `send` input atom: a key chord, `text:`, `paste:`, `raw:`, `raw-file:`, or one
/// printable character.
pub(super) fn input_atom_for_cli(atom: &str) -> Result<InputAtom> {
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

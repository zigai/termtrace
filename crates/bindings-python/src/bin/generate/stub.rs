use std::{error::Error, path::Path};

pub(crate) fn write(package: &Path) -> Result<String, Box<dyn Error>> {
    _termtrace::stub_info()?.generate()?;

    // Mixed layouts are written as a stub package; one file next to the extension is simpler.
    let generated = package.join("_termtrace");
    let mut stub = fs_err::read_to_string(generated.join("__init__.pyi"))?;
    fs_err::remove_file(generated.join("__init__.pyi"))?;
    fs_err::remove_dir(generated)?;

    // pyo3-stub-gen names every exception base as a builtin, including this module's own.
    let classes = stub
        .lines()
        .filter_map(|line| return line.strip_prefix("class "))
        .filter_map(|rest| return rest.split(['(', ':']).next())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for class in classes {
        stub = stub.replace(&format!("(builtins.{class})"), &format!("({class})"));
    }

    // Exit annotations come from the PyO3 signature, which takes optional `Any` arguments.
    stub = stub.replace(
        "# ruff: noqa: E501, F401, F403, F405",
        "# ruff: noqa: E501, F401, F403, F405, PYI036",
    );
    fs_err::write(package.join("_termtrace.pyi"), &stub)?;
    return Ok(stub);
}

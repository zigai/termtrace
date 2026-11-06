//! Write the Python package's native stub, model types, and asynchronous wrappers.

mod aio;
mod stub;
mod types;

use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("python/termtrace");
    let stub = stub::write(&package)?;
    types::write(&package, &stub)?;
    aio::write(&package, &stub)?;

    return Ok(());
}

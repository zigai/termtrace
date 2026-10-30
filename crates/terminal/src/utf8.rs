use std::str;

#[derive(Debug, Default)]
pub(crate) struct Utf8Prefix {
    bytes: Vec<u8>,
}

impl Utf8Prefix {
    pub(crate) fn push(&mut self, byte: u8) -> bool {
        let continuation = if self.bytes.is_empty() {
            false
        } else {
            self.bytes.push(byte);

            match str::from_utf8(&self.bytes) {
                Ok(_) => {
                    self.bytes.clear();
                    true
                }
                Err(error) if error.error_len().is_none() => true,
                Err(_) => {
                    self.bytes.clear();
                    false
                }
            }
        };

        if !continuation
            && str::from_utf8(&[byte]).is_err_and(|error| return error.error_len().is_none())
        {
            self.bytes.push(byte);
        }

        return continuation;
    }
}

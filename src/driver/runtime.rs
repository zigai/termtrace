use super::*;

/// Serve requests until clean input EOF.
pub fn serve(mut input: impl BufRead, mut output: impl Write) -> Result<()> {
    let mut driver = Driver::default();
    while let Some(frame) = read_frame(&mut input)? {
        let response = match frame {
            DriverFrame::Request(request) => driver.handle_request(request),
            DriverFrame::Invalid { line, column } => driver_error(
                Uuid::nil(),
                "invalid_request",
                format!(
                    "driver request does not match the protocol schema at line {line} column {column}"
                ),
                None,
            ),
            DriverFrame::Unterminated => driver_error(
                Uuid::nil(),
                "invalid_request",
                "driver request is not newline terminated",
                None,
            ),
        };
        serde_json::to_writer(&mut output, &response).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<driver-stdout>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        output
            .write_all(b"\n")
            .and_then(|()| return output.flush())
            .map_err(|source| {
                return Error::Filesystem {
                    path: PathBuf::from("<driver-stdout>"),
                    source,
                };
            })?;
    }
    return Ok(());
}

#[derive(Default)]
pub(super) struct Driver {
    pub(super) sessions: HashMap<Uuid, PathBuf>,
    pub(super) launched: HashSet<Uuid>,
    cached: HashMap<Uuid, CachedDriverResponse>,
    order: VecDeque<Uuid>,
}

struct CachedDriverResponse {
    method_hash: String,
    response: DriverResponse,
}

impl Driver {
    fn handle_request(&mut self, request: DriverRequest) -> DriverResponse {
        let method_hash = match serde_json::to_vec(&request.method) {
            Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
            Err(_) => {
                return driver_error(
                    request.request_id,
                    "invalid_request",
                    "driver request method could not be encoded",
                    None,
                );
            }
        };
        if let Some(cached) = self.cached.get(&request.request_id) {
            if cached.method_hash == method_hash {
                return cached.response.clone();
            }
            return driver_error(
                request.request_id,
                "request_id_collision",
                "driver request identifier was already used for a different method",
                None,
            );
        }
        let response = if request.version != DRIVER_PROTOCOL_VERSION {
            driver_error(
                request.request_id,
                "unsupported_version",
                format!("driver protocol version {} is unsupported", request.version),
                None,
            )
        } else {
            self.execute(&request)
        };
        if driver_response_is_cacheable(&response) {
            self.cache(request.request_id, method_hash, response.clone());
        }
        return response;
    }

    fn cache(&mut self, request_id: Uuid, method_hash: String, response: DriverResponse) {
        self.cached.insert(
            request_id,
            CachedDriverResponse {
                method_hash,
                response,
            },
        );
        self.order.push_back(request_id);
        while self.cached.len() > DRIVER_RESPONSE_CACHE {
            if let Some(evicted) = self.order.pop_front() {
                self.cached.remove(&evicted);
            }
        }
    }
}

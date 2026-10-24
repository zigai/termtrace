use super::execute::{execute, Completed, Failure};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rmcp::{
    model::{
        ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult, Resource,
        ResourceContents, ResourceTemplate,
    },
    service::RequestContext,
    ErrorData, RoleServer,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tempfile::TempDir;

const PREFIX: &str = "termtrace://sessions/";
const JSON: &str = "application/json";
const TEXT: &str = "text/plain";
const PNG: &str = "image/png";

#[derive(Clone, Copy)]
enum View {
    Status,
    Screen,
    Output,
    Screenshot,
}

impl View {
    const ALL: [Self; 4] = [Self::Status, Self::Screen, Self::Output, Self::Screenshot];

    fn suffix(self) -> &'static str {
        return match self {
            Self::Status => "",
            Self::Screen => "/screen",
            Self::Output => "/output",
            Self::Screenshot => "/screenshot",
        };
    }

    fn mime_type(self) -> &'static str {
        return match self {
            Self::Status => JSON,
            Self::Screen | Self::Output => TEXT,
            Self::Screenshot => PNG,
        };
    }

    fn title(self) -> &'static str {
        return match self {
            Self::Status => "Session status",
            Self::Screen => "Session screen",
            Self::Output => "Session output",
            Self::Screenshot => "Session screenshot",
        };
    }

    fn description(self) -> &'static str {
        return match self {
            Self::Status => "Lifecycle, command, and geometry of a session.",
            Self::Screen => "Text of the latest presented screen of a session.",
            Self::Output => {
                "The program's output with control sequences removed; needs a completed session."
            }
            Self::Screenshot => {
                "PNG of the last presented screen, rendered from the recorded terminal state; \
                 needs a completed session."
            }
        };
    }

    fn uri(self, session: &str) -> String {
        return format!("{PREFIX}{session}{}", self.suffix());
    }

    fn parse(uri: &str) -> Option<(Self, &str)> {
        let path = uri.strip_prefix(PREFIX)?;

        return Self::ALL.into_iter().find_map(|view| {
            let session = path.strip_suffix(view.suffix())?;

            return (!session.is_empty() && !session.contains('/')).then_some((view, session));
        });
    }
}

#[derive(Deserialize)]
struct ListedSession {
    alias: String,
    lifecycle: String,
}

pub(super) fn templates() -> ListResourceTemplatesResult {
    return ListResourceTemplatesResult::with_all_items(
        View::ALL
            .into_iter()
            .map(|view| {
                let name = format!("session{}", view.suffix().replace('/', "-"));

                return ResourceTemplate::new(view.uri("{session}"), name)
                    .with_title(view.title())
                    .with_description(view.description())
                    .with_mime_type(view.mime_type());
            })
            .collect(),
    );
}

pub(super) async fn list(
    context: &RequestContext<RoleServer>,
) -> Result<ListResourcesResult, ErrorData> {
    let arguments = ["status".to_owned(), "--json".to_owned()];
    let completed = run(&arguments, context).await?;
    let sessions = serde_json::from_slice::<Vec<ListedSession>>(&completed.stdout)
        .map_err(|error| return ErrorData::internal_error(error.to_string(), None))?;

    let resources = sessions
        .iter()
        .flat_map(|session| {
            return [View::Status, View::Screen].map(|view| {
                return Resource::new(view.uri(&session.alias), session.alias.clone())
                    .with_title(format!("{} ({})", view.title(), session.alias))
                    .with_description(format!(
                        "{} Lifecycle: {}.",
                        view.description(),
                        session.lifecycle
                    ))
                    .with_mime_type(view.mime_type());
            });
        })
        .collect();

    return Ok(ListResourcesResult::with_all_items(resources));
}

pub(super) async fn read(
    uri: &str,
    context: &RequestContext<RoleServer>,
) -> Result<ReadResourceResult, ErrorData> {
    let Some((view, session)) = View::parse(uri) else {
        return Err(ErrorData::resource_not_found(
            "unknown termtrace resource",
            Some(json!({ "uri": uri })),
        ));
    };
    let contents = match view {
        View::Status => {
            let arguments = ["status", session, "--json"].map(str::to_owned);
            let completed = run(&arguments, context).await?;

            ResourceContents::text(String::from_utf8_lossy(&completed.stdout), uri)
                .with_mime_type(JSON)
        }
        View::Screen => {
            let arguments = ["screen", session, "--json"].map(str::to_owned);
            let completed = run(&arguments, context).await?;
            let screen = serde_json::from_slice::<Value>(&completed.stdout)
                .ok()
                .and_then(|value| return value.get("screen")?.as_str().map(str::to_owned))
                .ok_or_else(|| {
                    return ErrorData::internal_error("screen output has no screen text", None);
                })?;

            ResourceContents::text(screen, uri)
        }
        View::Output => {
            let arguments = ["analyze", "output", session, "--strip-controls"].map(str::to_owned);
            let completed = run(&arguments, context).await?;

            ResourceContents::text(String::from_utf8_lossy(&completed.stdout), uri)
        }
        View::Screenshot => {
            let directory = TempDir::new()
                .map_err(|error| return ErrorData::internal_error(error.to_string(), None))?;
            let destination = directory.path().join("screenshot");
            let arguments = [
                "screenshot".to_owned(),
                session.to_owned(),
                "--json".to_owned(),
                format!("--out={}", destination.display()),
            ];
            let completed = run(&arguments, context).await?;
            let png = serde_json::from_slice::<Value>(&completed.stdout)
                .map_err(|error| return error.to_string())
                .and_then(|output| return super::screenshot_png(&output))
                .map_err(|message| return ErrorData::internal_error(message, None))?;

            ResourceContents::blob(STANDARD.encode(png), uri).with_mime_type(PNG)
        }
    };

    return Ok(ReadResourceResult::new(vec![contents]));
}

async fn run(
    arguments: &[String],
    context: &RequestContext<RoleServer>,
) -> Result<Completed, ErrorData> {
    return match execute(arguments, context).await {
        Ok(completed) if completed.succeeded => Ok(completed),
        Ok(completed) => Err(ErrorData::resource_not_found(completed.stderr, None)),
        Err(Failure::Cancelled) => Err(ErrorData::internal_error("request cancelled", None)),
        Err(Failure::Unavailable(message)) => Err(ErrorData::internal_error(message, None)),
    };
}

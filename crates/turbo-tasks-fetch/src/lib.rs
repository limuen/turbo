#![feature(min_specialization)]
#![feature(arbitrary_self_types)]
#![feature(async_fn_in_trait)]

use anyhow::Result;
use turbo_tasks::Vc;
use turbo_tasks_fs::FileSystemPath;
use turbopack_core::issue::{Issue, IssueSeverity, StyledString};

pub fn register() {
    turbo_tasks::register();
    turbo_tasks_fs::register();
    turbopack_core::register();
    include!(concat!(env!("OUT_DIR"), "/register.rs"));
}

#[turbo_tasks::value(transparent)]
pub struct FetchResult(Result<Vc<HttpResponse>, Vc<FetchError>>);

#[turbo_tasks::value(shared)]
#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vc<HttpResponseBody>,
}

#[turbo_tasks::value(shared)]
#[derive(Debug)]
pub struct HttpResponseBody(pub Vec<u8>);

#[turbo_tasks::value_impl]
impl HttpResponseBody {
    #[turbo_tasks::function]
    pub async fn to_string(self: Vc<Self>) -> Result<Vc<String>> {
        let this = &*self.await?;
        Ok(Vc::cell(std::str::from_utf8(&this.0)?.to_owned()))
    }
}

#[turbo_tasks::function]
pub async fn fetch(url: Vc<String>, user_agent: Vc<Option<String>>) -> Result<Vc<FetchResult>> {
    let url = &*url.await?;
    let user_agent = &*user_agent.await?;
    let client = reqwest::Client::new();

    let mut builder = client.get(url);
    if let Some(user_agent) = user_agent {
        builder = builder.header("User-Agent", user_agent);
    }

    let response = builder.send().await.and_then(|r| r.error_for_status());
    match response {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.bytes().await?.to_vec();

            Ok(Vc::cell(Ok(HttpResponse {
                status,
                body: HttpResponseBody::cell(HttpResponseBody(body)),
            }
            .cell())))
        }
        Err(err) => Ok(Vc::cell(Err(
            FetchError::from_reqwest_error(&err, url).cell()
        ))),
    }
}

#[derive(Debug)]
#[turbo_tasks::value(shared)]
pub enum FetchErrorKind {
    Connect,
    Timeout,
    Status(u16),
    Other,
}

#[turbo_tasks::value(shared)]
pub struct FetchError {
    pub url: Vc<String>,
    pub kind: Vc<FetchErrorKind>,
    pub detail: Vc<String>,
}

impl FetchError {
    fn from_reqwest_error(error: &reqwest::Error, url: &str) -> FetchError {
        let kind = if error.is_connect() {
            FetchErrorKind::Connect
        } else if error.is_timeout() {
            FetchErrorKind::Timeout
        } else if let Some(status) = error.status() {
            FetchErrorKind::Status(status.as_u16())
        } else {
            FetchErrorKind::Other
        };

        FetchError {
            detail: Vc::cell(error.to_string()),
            url: Vc::cell(url.to_owned()),
            kind: kind.into(),
        }
    }
}

#[turbo_tasks::value_impl]
impl FetchError {
    #[turbo_tasks::function]
    pub async fn to_issue(
        self: Vc<Self>,
        severity: Vc<IssueSeverity>,
        issue_context: Vc<FileSystemPath>,
    ) -> Result<Vc<FetchIssue>> {
        let this = &*self.await?;
        Ok(FetchIssue {
            issue_context,
            severity,
            url: this.url,
            kind: this.kind,
            detail: this.detail,
        }
        .into())
    }
}

#[turbo_tasks::value(shared)]
pub struct FetchIssue {
    pub issue_context: Vc<FileSystemPath>,
    pub severity: Vc<IssueSeverity>,
    pub url: Vc<String>,
    pub kind: Vc<FetchErrorKind>,
    pub detail: Vc<String>,
}

#[turbo_tasks::value_impl]
impl Issue for FetchIssue {
    #[turbo_tasks::function]
    fn file_path(&self) -> Vc<FileSystemPath> {
        self.issue_context
    }

    #[turbo_tasks::function]
    fn severity(&self) -> Vc<IssueSeverity> {
        self.severity
    }

    #[turbo_tasks::function]
    fn title(&self) -> Vc<String> {
        Vc::cell("Error while requesting resource".to_string())
    }

    #[turbo_tasks::function]
    fn category(&self) -> Vc<String> {
        Vc::cell("fetch".to_string())
    }

    #[turbo_tasks::function]
    async fn description(&self) -> Result<Vc<StyledString>> {
        let url = &*self.url.await?;
        let kind = &*self.kind.await?;

        Ok(StyledString::Text(match kind {
            FetchErrorKind::Connect => format!(
                "There was an issue establishing a connection while requesting {}.",
                url
            ),
            FetchErrorKind::Status(status) => {
                format!(
                    "Received response with status {} when requesting {}",
                    status, url
                )
            }
            FetchErrorKind::Timeout => format!("Connection timed out when requesting {}", url),
            FetchErrorKind::Other => format!("There was an issue requesting {}", url),
        })
        .cell())
    }

    #[turbo_tasks::function]
    fn detail(&self) -> Vc<String> {
        self.detail
    }
}

//! HTTP client for the daemon's control API.

use anyhow::Context as _;
use serde::de::DeserializeOwned;

/// Authenticated client for the control API.
///
/// Target resolution: `--url` / `SPACEBOT_URL` selects a remote instance;
/// otherwise the base URL and token come from the local config. Token
/// resolution: `--token` / `SPACEBOT_TOKEN`, then `api.auth_token`.
pub struct ApiClient {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
    /// Instance dir of the local daemon, when targeting it via config.
    /// Enables the daemon-down hint on connection failures.
    local_instance_dir: Option<std::path::PathBuf>,
}

impl ApiClient {
    pub fn from_context(ctx: &super::Context) -> anyhow::Result<Self> {
        let (base, token, local_instance_dir) = match &ctx.url {
            Some(url) => (
                url.trim_end_matches('/').to_string(),
                ctx.token.clone(),
                None,
            ),
            None => {
                let config = super::load_config(&ctx.config_path)?;
                let base = format!("http://{}:{}", config.api.bind, config.api.port);
                let token = ctx.token.clone().or_else(|| config.api.auth_token.clone());
                (base, token, Some(config.instance_dir.clone()))
            }
        };

        Ok(Self {
            http: reqwest::Client::new(),
            base,
            token,
            local_instance_dir,
        })
    }

    pub async fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::GET, path, None).await
    }

    pub async fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    pub async fn put(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::PUT, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::DELETE, path, None).await
    }

    /// DELETE with a JSON body — the bindings and messaging-instance
    /// delete endpoints match on the request body.
    pub async fn delete_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.send(reqwest::Method::DELETE, path, Some(body)).await
    }

    pub async fn post_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> anyhow::Result<serde_json::Value> {
        let mut request = self.http.post(self.url_for(path)).multipart(form);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        self.execute(request).await
    }

    /// Open a streaming GET request (SSE endpoints). The caller consumes
    /// the byte stream; connection errors get the same daemon-down hint.
    pub async fn stream(&self, path: &str) -> anyhow::Result<reqwest::Response> {
        let mut request = self.http.get(self.url_for(path));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| self.connect_hint(error))?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("API request failed ({status})");
        }
        Ok(response)
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let mut request = self.http.request(method, self.url_for(path));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        self.execute(request).await
    }

    fn url_for(&self, path: &str) -> String {
        format!("{}/api/{}", self.base, path.trim_start_matches('/'))
    }

    /// Send a prepared request and map the response to JSON with the shared
    /// status/error-body handling.
    async fn execute(&self, request: reqwest::RequestBuilder) -> anyhow::Result<serde_json::Value> {
        let response = request
            .send()
            .await
            .map_err(|error| self.connect_hint(error))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .context("failed to read API response")?;

        let value: serde_json::Value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .with_context(|| format!("API returned a non-JSON response ({status})"))?
        };

        if !status.is_success() {
            let message = value
                .get("error")
                .and_then(|e| e.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("API request failed ({status})"));
            anyhow::bail!("{message}");
        }

        Ok(value)
    }

    /// Turn a connection failure into an actionable message. Distinguishes
    /// "daemon not running" from "daemon up but API unreachable" via the
    /// daemon socket when targeting the local instance.
    fn connect_hint(&self, error: reqwest::Error) -> anyhow::Error {
        if !error.is_connect() {
            return anyhow::Error::from(error).context("API request failed");
        }

        match &self.local_instance_dir {
            Some(instance_dir) => {
                let paths = spacebot::daemon::DaemonPaths::new(instance_dir);
                if spacebot::daemon::is_running(&paths).is_none() {
                    anyhow::anyhow!("spacebot is not running — start it with `spacebot start`")
                } else {
                    anyhow::anyhow!(
                        "the daemon is running but the API at {} is unreachable — check [api] bind/port in config.toml",
                        self.base
                    )
                }
            }
            None => anyhow::anyhow!("cannot reach the spacebot API at {}", self.base),
        }
    }
}

/// Deserialize an API response into its typed shape.
pub fn parse<T: DeserializeOwned>(value: serde_json::Value) -> anyhow::Result<T> {
    serde_json::from_value(value).context("unexpected API response shape")
}

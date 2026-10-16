//! Estado da nuvem do Ollama local (`/api/status`), lido pela CLI
//! (`fastwisper ollama`, `doctor`) e pelo hub. Só consulta; quem liga ou
//! desliga é a CLI, a pedido.
use std::time::Duration;

/// O que o `/api/status` do Ollama informa sobre a nuvem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudStatus {
    pub disabled: bool,
    /// `env`, `config` ou `none`.
    pub source: String,
}

impl CloudStatus {
    pub fn describe(&self) -> String {
        let state = if self.disabled { "desligada" } else { "ligada" };
        match self.source.as_str() {
            "env" => format!("nuvem {state} (OLLAMA_NO_CLOUD)"),
            "config" => format!("nuvem {state} (server.json do Ollama)"),
            _ => format!("nuvem {state}"),
        }
    }
}

pub fn parse_status(body: &str) -> Option<CloudStatus> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let cloud = json.get("cloud")?;
    Some(CloudStatus {
        disabled: cloud.get("disabled")?.as_bool()?,
        source: cloud
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("none")
            .to_owned(),
    })
}

/// `GET /api/status` do Ollama local (sem proxy nem redirect).
pub fn cloud_status(endpoint: &str) -> Result<CloudStatus, String> {
    let url = format!("{}/api/status", endpoint.trim_end_matches('/'));
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut res = agent
        .get(&url)
        .call()
        .map_err(|_| "Ollama não respondeu".to_owned())?;
    if !res.status().is_success() {
        return Err(format!(
            "Ollama não informa o estado da nuvem (HTTP {}; versão antiga?)",
            res.status().as_u16()
        ));
    }
    let body = res
        .body_mut()
        .read_to_string()
        .map_err(|_| "resposta ilegível do Ollama".to_owned())?;
    parse_status(&body).ok_or_else(|| "resposta inesperada de /api/status".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_parsed() {
        assert_eq!(
            parse_status(r#"{"cloud":{"disabled":true,"source":"env"}}"#),
            Some(CloudStatus {
                disabled: true,
                source: "env".into()
            })
        );
        assert_eq!(
            parse_status(r#"{"cloud":{"disabled":false}}"#).map(|s| s.source),
            Some("none".into())
        );
        assert_eq!(parse_status(r#"{"version":"0.1"}"#), None);
        assert_eq!(parse_status("não é json"), None);
    }
}

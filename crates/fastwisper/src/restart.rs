//! `fastwisper restart`: reinicia o daemon pelo serviço do usuário, para
//! valerem as chaves que só são lidas no início (`asr.model`, `llm.endpoint`...).
use std::process::Command;

use anyhow::{Context, bail};

pub const UNIT: &str = "fastwisperd.service";

pub fn run() -> anyhow::Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "restart", UNIT])
        .status()
        .context("não foi possível executar systemctl")?;
    if !status.success() {
        bail!(
            "`systemctl --user restart {UNIT}` falhou; se o daemon não roda como serviço, feche e abra o `fastwisperd`"
        );
    }
    crate::human_print!("FastWisper reiniciado.");
    Ok(())
}

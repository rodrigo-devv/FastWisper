//! Apoio aos testes: scripts falsos executáveis sem corrida de ETXTBSY.

use std::path::Path;

/// Grava `content` em `path` como executável. Quem escreve é um processo
/// `cp` à parte: se o próprio teste abrisse o arquivo para escrita, um
/// `fork` de outro teste em paralelo herdaria esse descritor até o `exec`,
/// e executar o script nesse instante falharia com ETXTBSY.
pub fn write_executable(path: &Path, content: &str) {
    let source = path.with_extension("fonte");
    std::fs::write(&source, content).expect("fonte do script");
    let status = std::process::Command::new("cp")
        .arg(&source)
        .arg(path)
        .status()
        .expect("cp");
    assert!(status.success(), "cp falhou");
    let status = std::process::Command::new("chmod")
        .arg("755")
        .arg(path)
        .status()
        .expect("chmod");
    assert!(status.success(), "chmod falhou");
}

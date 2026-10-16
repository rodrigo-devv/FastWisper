//! `fastwisper hub <página>`: abre (ou foca) o hub direto num destino. Só
//! destinos desta lista; qualquer outro argumento é ignorado. As páginas do
//! Hub v1 continuam valendo e caem no lugar novo.
use std::sync::Mutex;

use serde::Serialize;
use tauri::State;

/// Uma página do hub ou uma aba de Configurações (um modal aberto sobre a
/// página atual, não uma rota).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    Page { route: &'static str },
    Settings { tab: &'static str },
}

const fn page(route: &'static str) -> Destination {
    Destination::Page { route }
}

const fn settings(tab: &'static str) -> Destination {
    Destination::Settings { tab }
}

pub const PAGES: [(&str, Destination); 21] = [
    ("inicio", page("/")),
    ("notas", page("/notas")),
    ("historico", page("/historico")),
    ("temas", page("/temas")),
    ("personalizacao", page("/personalizacao/dicionario")),
    ("dicionario", page("/personalizacao/dicionario")),
    ("snippets", page("/personalizacao/snippets")),
    ("estilo", page("/personalizacao/estilo")),
    ("configuracoes", settings("geral")),
    ("geral", settings("geral")),
    ("sistema", settings("sistema")),
    ("vibe-coding", settings("vibe-coding")),
    ("experimental", settings("experimental")),
    ("privacidade", settings("privacidade")),
    ("ajuda", settings("ajuda")),
    ("sobre", settings("sobre")),
    // Páginas do Hub v1.
    ("microfone", settings("geral")),
    ("transcricao", settings("sistema")),
    ("ia", settings("sistema")),
    ("desempenho", settings("sistema")),
    // Terminais ficam em Vibe coding; o clipboard, em Dados e privacidade.
    ("colagem", settings("vibe-coding")),
];

/// Destino de `--page <nome>` nos argumentos do processo.
pub fn destination_from_args(args: &[String]) -> Option<Destination> {
    let at = args.iter().position(|a| a == "--page")?;
    let name = args.get(at + 1)?;
    PAGES
        .iter()
        .find_map(|(page, destination)| (page == name).then_some(*destination))
}

/// Destino pedido na abertura, entregue uma vez à webview.
#[derive(Default)]
pub struct InitialDestination(pub Mutex<Option<Destination>>);

#[tauri::command]
pub fn initial_destination(state: State<'_, InitialDestination>) -> Option<Destination> {
    state.0.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Abas do modal de Configurações (as mesmas de `src/lib/navigation.ts`).
    const SETTINGS_TABS: [&str; 7] = [
        "geral",
        "sistema",
        "vibe-coding",
        "experimental",
        "privacidade",
        "ajuda",
        "sobre",
    ];

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn open(name: &str) -> Option<Destination> {
        destination_from_args(&args(&["hub", "--page", name]))
    }

    #[test]
    fn old_pages_land_on_the_new_destinations() {
        // Páginas do Hub v1 → destinos do Hub v2.
        for (old, new) in [
            ("configuracoes", settings("geral")),
            ("microfone", settings("geral")),
            ("transcricao", settings("sistema")),
            ("ia", settings("sistema")),
            ("desempenho", settings("sistema")),
            ("privacidade", settings("privacidade")),
            ("ajuda", settings("ajuda")),
            ("sobre", settings("sobre")),
            ("colagem", settings("vibe-coding")),
            ("dicionario", page("/personalizacao/dicionario")),
            ("inicio", page("/")),
            ("historico", page("/historico")),
            ("temas", page("/temas")),
        ] {
            assert_eq!(open(old), Some(new), "{old}");
        }
    }

    #[test]
    fn new_names_are_accepted() {
        for tab in SETTINGS_TABS {
            assert_eq!(open(tab), Some(settings(tab)), "{tab}");
        }
        assert_eq!(open("temas"), Some(page("/temas")));
        assert_eq!(open("notas"), Some(page("/notas")));
        assert_eq!(open("snippets"), Some(page("/personalizacao/snippets")));
        assert_eq!(open("estilo"), Some(page("/personalizacao/estilo")));
    }

    #[test]
    fn anything_else_is_ignored() {
        for bad in [
            &["hub"][..],
            &["hub", "--page"],
            &["hub", "--page", "../etc"],
            &["hub", "--page", "https://x"],
            &["hub", "--page", "/historico"],
            &["hub", "--page", "Geral"],
        ] {
            assert_eq!(destination_from_args(&args(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn destinations_reach_the_webview_as_tagged_json() {
        assert_eq!(
            serde_json::to_value(page("/notas")).expect("json"),
            serde_json::json!({ "kind": "page", "route": "/notas" })
        );
        assert_eq!(
            serde_json::to_value(settings("sistema")).expect("json"),
            serde_json::json!({ "kind": "settings", "tab": "sistema" })
        );
    }

    #[test]
    fn every_destination_exists_in_the_hub_and_in_the_cli() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let routes = root.join("../src/routes");
        let frontend =
            std::fs::read_to_string(root.join("../src/lib/navigation.ts")).expect("navigation.ts");
        for (name, destination) in PAGES {
            match destination {
                Destination::Page { route } => {
                    let page = routes
                        .join(route.trim_start_matches('/'))
                        .join("+page.svelte");
                    assert!(page.is_file(), "{name}: {} não existe", page.display());
                }
                Destination::Settings { tab } => {
                    assert!(SETTINGS_TABS.contains(&tab), "{name}: aba {tab}");
                }
            }
        }
        for tab in SETTINGS_TABS {
            assert!(frontend.contains(&format!("'{tab}'")), "{tab} falta no hub");
        }
        // `fastwisper hub <página>` aceita exatamente estas páginas.
        let cli = std::fs::read_to_string(root.join("../../crates/fastwisper/src/hub.rs"))
            .expect("hub.rs da CLI");
        let start = cli.find("pub const PAGES").expect("PAGES na CLI");
        let list = &cli[start..start + cli[start..].find("];").expect("fim de PAGES")];
        let mut names: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
        let mut ours: Vec<&str> = PAGES.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        ours.sort_unstable();
        assert_eq!(names, ours, "lista da CLI diferente da do hub");
    }
}

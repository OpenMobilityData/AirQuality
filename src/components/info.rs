use leptos::prelude::*;

use crate::i18n::Lang;

/// Long-form background content, authored as semantic HTML fragments under
/// `src/content/` (without styling) and embedded at compile time. Styling comes
/// entirely from `.info-page` rules in `style.css`. External attribution links
/// open in a new tab; in-page footnote/`#ref` anchors resolve within the
/// scrolling article.
const NETWORK_EN: &str = include_str!("../content/rsqa-en.html");
const NETWORK_FR: &str = include_str!("../content/rsqa-fr.html");
const METHODS_EN: &str = include_str!("../content/rsqa-methods-en.html");
const METHODS_FR: &str = include_str!("../content/rsqa-methods-fr.html");

/// Which informational article to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoKind {
    /// Background on the RSQA monitoring network (history, coverage, pollutants).
    Network,
    /// This site's data sources and processing methodology.
    Methods,
}

#[component]
pub fn InfoPage(kind: InfoKind) -> impl IntoView {
    let lang = use_context::<ReadSignal<Lang>>().expect("Lang context not provided");
    let html = move || match (kind, lang.get()) {
        (InfoKind::Network, Lang::En) => NETWORK_EN,
        (InfoKind::Network, Lang::Fr) => NETWORK_FR,
        (InfoKind::Methods, Lang::En) => METHODS_EN,
        (InfoKind::Methods, Lang::Fr) => METHODS_FR,
    };
    view! {
        <div class="info-page">
            <article class="info-content" inner_html=html />
        </div>
    }
}

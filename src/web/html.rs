//! Kleine, handgerollte HTML-Bausteine für das Web-Dashboard.
//! Kein Template-Engine, kein JavaScript — nur Strings mit konsequentem Escaping.

/// HTML-Sonderzeichen escapen. Muss auf ALLE Nutzereingaben und DB-Inhalte
/// angewendet werden, bevor sie in eine Seite eingebettet werden.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Preis wie im CLI formatieren: "1.39 €".
pub fn price(p: Option<f64>) -> String {
    match p {
        Some(p) => format!("{p:.2} €"),
        None => "–".to_string(),
    }
}

const STYLE: &str = "
body { font-family: system-ui, sans-serif; margin: 0; color: #222; background: #fafafa; }
nav { background: #2d6a4f; padding: 0.6rem 1rem; }
nav a { color: #fff; text-decoration: none; margin-right: 1.2rem; font-weight: 600; }
nav a:hover { text-decoration: underline; }
main { max-width: 60rem; margin: 0 auto; padding: 1rem; }
h1 { font-size: 1.4rem; } h2 { font-size: 1.1rem; margin-top: 1.5rem; }
table { border-collapse: collapse; width: 100%; background: #fff; }
th, td { text-align: left; padding: 0.4rem 0.6rem; border-bottom: 1px solid #ddd; }
th { background: #eee; }
td.num, th.num { text-align: right; }
form.inline { display: inline; }
input, button { font: inherit; padding: 0.25rem 0.5rem; }
button { background: #2d6a4f; color: #fff; border: none; border-radius: 3px; cursor: pointer; }
.hinweis { color: #666; }
svg.sparkline { vertical-align: middle; }
";

/// Komplette Seite mit Navigation und eingebettetem CSS rendern.
/// `body` muss bereits fertiges (escaptes) HTML sein.
pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"de\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{} — lechariot</title><style>{STYLE}</style></head><body>\
<nav><a href=\"/\">Übersicht</a><a href=\"/search\">Suche</a>\
<a href=\"/compare\">Vergleich</a><a href=\"/watchlist\">Watchlist</a></nav>\
<main><h1>{}</h1>\n{body}\n</main></body></html>",
        escape(title),
        escape(title),
    )
}

/// Tabelle rendern. Zellen müssen bereits fertiges (escaptes) HTML sein.
/// Header mit führendem '>' werden rechtsbündig gesetzt (Zahlenspalten).
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut html = String::from("<table><thead><tr>");
    let mut numeric = Vec::new();
    for h in headers {
        let (class, label) = match h.strip_prefix('>') {
            Some(rest) => (" class=\"num\"", rest),
            None => ("", *h),
        };
        numeric.push(class);
        html.push_str(&format!("<th{class}>{}</th>", escape(label)));
    }
    html.push_str("</tr></thead><tbody>");
    for row in rows {
        html.push_str("<tr>");
        for (i, cell) in row.iter().enumerate() {
            let class = numeric.get(i).copied().unwrap_or("");
            html.push_str(&format!("<td{class}>{cell}</td>"));
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    html
}

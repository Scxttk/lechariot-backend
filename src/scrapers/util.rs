use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Gemeinsamer HTTP-Helfer für Akamai-geschützte Seiten (Netto, ALDI Süd).
//
// Der Akamai-Bot-Schutz fingerprintet den TLS-Stack — reqwest/rustls wird
// konsequent mit HTTP 403 geblockt, während curl mit vollem
// Browser-Header-Satz durchkommt (verifiziert 2026-07). Deshalb laufen
// diese Requests über das System-curl (Präzedenzfall: rewe.rs shellt zum
// rewerse-CLI aus), mit Retries gegen sporadische 403.

pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const RETRIES: u32 = 3;

// Nominatim ist der einzige Host hier, der uns aus Kulanz bedient, und seine
// Nutzungsbedingungen verlangen zwei Dinge: einen UA, der sagt wer ruft, und
// höchstens einen Request pro Sekunde.
//
// Beides war bis 2026-07-30 nicht erfüllt, obwohl der Modulkommentar von
// `store_finder` das Gegenteil behauptete: Die Geocoding-Requests liefen über
// `blocking_client()` mit dem Chrome-UA oben, und `polite_pause` wartete
// 300-800 ms. Aufgefallen ist es beim Einbau des **zweiten**
// Nominatim-Endpunkts (`/reverse` für die Gebiets-Anforderung).
//
// Die Sekunde steht deshalb hier in `polite_pause` und nicht an der
// Aufrufstelle: Ein dritter Endpunkt kann sie so nicht vergessen.
pub const NOMINATIM_HOST: &str = "nominatim.openstreetmap.org";
pub const NOMINATIM_USER_AGENT: &str =
    "LeChariot/1.0 (+https://github.com/Scxttk/lechariot-backend)";
const NOMINATIM_MIN_GAP_MS: u64 = 1100;
const DEFAULT_MIN_GAP_MS: u64 = 300;

// Fehlerkontext einheitlich über alle Scraper: Kette, Schritt, URL.
pub fn ctx(chain: &str, step: &str, url: &str) -> String {
    format!("[{chain}] {step} fehlgeschlagen: {url}")
}

// reqwest-Clients mit dem gemeinsamen Browser-User-Agent.
pub fn async_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("HTTP-Client konnte nicht erstellt werden")
}

pub fn blocking_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("HTTP-Client konnte nicht erstellt werden")
}

// Client für Nominatim — eigener UA laut OSM-Policy, siehe oben.
pub fn nominatim_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(NOMINATIM_USER_AGENT)
        .build()
        .context("HTTP-Client konnte nicht erstellt werden")
}

// Mindestabstand zweier Requests an denselben Host, bevor der Jitter
// dazukommt. Nominatim bekommt seine Sekunde, alle anderen die bisherigen
// 300 ms.
pub fn min_gap_ms(host: &str) -> u64 {
    if host == NOMINATIM_HOST { NOMINATIM_MIN_GAP_MS } else { DEFAULT_MIN_GAP_MS }
}

// Höfliches Rate-Limiting: vor aufeinanderfolgenden Requests an denselben
// Host eine kleine, zufällig gestreute Pause einlegen (Nominatim 1100-1600 ms,
// sonst 300-800 ms). Erster Request an einen Host läuft ohne Verzögerung.
static LAST_REQUEST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

pub fn polite_pause(url: &str) {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string();

    let wait = {
        let guard = LAST_REQUEST.lock().unwrap();
        guard.as_ref().and_then(|map| map.get(&host)).and_then(|last| {
            let delay = Duration::from_millis(min_gap_ms(&host) + jitter_ms(500));
            delay.checked_sub(last.elapsed())
        })
    };
    if let Some(d) = wait {
        std::thread::sleep(d);
    }
    let mut guard = LAST_REQUEST.lock().unwrap();
    guard.get_or_insert_with(HashMap::new).insert(host, Instant::now());
}

// Pseudozufall aus der Systemuhr — reicht für Jitter, spart die rand-Dependency.
fn jitter_ms(range: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % range)
        .unwrap_or(0)
}

// GET über System-curl; liefert den Response-Body als String (nur bei HTTP 200).
pub fn curl_get(url: &str, extra_headers: &[(&str, &str)]) -> Result<String> {
    for attempt in 0..RETRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        polite_pause(url);
        let mut cmd = std::process::Command::new("curl");
        cmd.arg("-s")
            .arg("-L")
            .arg("--compressed")
            .arg("--max-time")
            .arg("30")
            .arg("-w")
            .arg("\n%{http_code}")
            .args(["-H", &format!("User-Agent: {USER_AGENT}")])
            .args(["-H", "Accept-Language: de-DE,de;q=0.9,en;q=0.8"])
            .args(["-H", "Accept-Encoding: gzip, deflate, br"]);
        for (k, v) in extra_headers {
            cmd.args(["-H", &format!("{k}: {v}")]);
        }
        let output = cmd
            .arg(url)
            .output()
            .context("curl nicht gefunden — wird für diesen Scraper benötigt")?;

        let body = String::from_utf8_lossy(&output.stdout);
        let (content, status) = match body.rsplit_once('\n') {
            Some((c, s)) => (c, s.trim()),
            None => continue,
        };
        if status == "200" {
            return Ok(content.to_string());
        }
    }
    bail!("Wiederholte Fehler für {url} (Akamai-Blockade?)")
}

/// Wie eine Antwort ohne Redirect-Ziel zu lesen ist.
///
/// Der Unterschied ist der ganze Punkt: „Die Seite gibt es nicht" ist ein
/// Ergebnis, „ich wurde geblockt" ist ein Ausfall. Bis 2026-07-31 sahen beide
/// Fälle gleich aus — leeres `%{redirect_url}` — und wurden gleich behandelt:
/// dreimal versuchen, dann dieselbe Meldung. Damit war ein echter 404 dreimal
/// so langsam wie nötig und eine Akamai-Blockade nicht als solche zu erkennen.
#[derive(Debug, Clone, PartialEq)]
pub enum RedirectOutcome {
    /// 3xx mit `Location` — der Normalfall.
    Target(String),
    /// 404/410: Diese Marktseite gibt es nicht. Nicht wiederholen, sie kommt
    /// beim zweiten Versuch auch nicht wieder.
    Gone(String),
    /// Alles andere: 403/429/5xx, `000` (Verbindung kam nicht zustande) und
    /// **200 ohne Location** — Akamai beantwortet eine geblockte Anfrage mit
    /// einer Challenge-Seite, nicht mit einem Fehlerstatus. Wiederholen lohnt.
    Blocked(String),
}

/// Die Einteilung als reine Funktion, damit ein Test sie ohne Netz fassen kann.
pub fn redirect_outcome(status: &str, target: &str) -> RedirectOutcome {
    let target = target.trim();
    if !target.is_empty() {
        return RedirectOutcome::Target(target.to_string());
    }
    match status.trim() {
        "404" | "410" => RedirectOutcome::Gone(status.trim().to_string()),
        other => RedirectOutcome::Blocked(other.to_string()),
    }
}

/// Wartezeiten zwischen den Versuchen, in Millisekunden. Begrenzt und
/// aufsteigend: Der gemeldete Ausfall (2026-07-30, EDEKA Böse in Ahlbeck) hat
/// die bisherigen 3 s × 2 zweimal überdauert, und eine IP-Blockade bei Akamai
/// hält länger als eine Sekunde. Länger als 13 s insgesamt darf es nicht
/// werden — der Redirect fällt je Filiale an, und ein Gebiet hat ein Dutzend.
pub const REDIRECT_BACKOFF_MS: &[u64] = &[1_000, 3_000, 9_000];

/// GET ohne Redirect-Folgen; liefert das Redirect-Ziel (Location) einer
/// 3xx-Antwort.
///
/// Wiederholt **nur** bei [`RedirectOutcome::Blocked`], mit wachsender Pause.
/// Ein 404 kommt sofort zurück: Papier über einen echten Fehler wäre genau
/// das, was hier nicht passieren soll.
pub fn curl_redirect_url(url: &str, extra_headers: &[(&str, &str)]) -> Result<String> {
    curl_redirect_url_with_backoff(url, extra_headers, REDIRECT_BACKOFF_MS)
}

/// Wie [`curl_redirect_url`], nur mit einstellbaren Pausen — der Test soll die
/// Wiederholungen zählen, nicht 13 Sekunden schlafen.
pub fn curl_redirect_url_with_backoff(
    url: &str,
    extra_headers: &[(&str, &str)],
    backoff_ms: &[u64],
) -> Result<String> {
    let mut last = RedirectOutcome::Blocked("000".to_string());
    for attempt in 0..=backoff_ms.len() {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(backoff_ms[attempt - 1]));
        }
        polite_pause(url);
        let mut cmd = std::process::Command::new("curl");
        cmd.arg("-s")
            .arg("-o")
            .arg(if cfg!(windows) { "NUL" } else { "/dev/null" })
            .arg("--max-time")
            .arg("30")
            .arg("-w")
            .arg("%{http_code} %{redirect_url}")
            .args(["-H", &format!("User-Agent: {USER_AGENT}")])
            .args(["-H", "Accept-Language: de-DE,de;q=0.9,en;q=0.8"])
            .args(["-H", "Accept-Encoding: gzip, deflate, br"]);
        for (k, v) in extra_headers {
            cmd.args(["-H", &format!("{k}: {v}")]);
        }
        let output = cmd
            .arg(url)
            .output()
            .context("curl nicht gefunden — wird für diesen Scraper benötigt")?;

        let written = String::from_utf8_lossy(&output.stdout);
        let (status, target) = written.trim().split_once(' ').unwrap_or((written.trim(), ""));
        last = redirect_outcome(status, target);
        match &last {
            RedirectOutcome::Target(t) => return Ok(t.clone()),
            // Ein 404 ist eine Antwort, kein Ausfall — sofort zurück.
            RedirectOutcome::Gone(status) => {
                bail!("{url} beantwortet den Markt-Redirect mit HTTP {status} — diese Marktseite gibt es nicht")
            }
            RedirectOutcome::Blocked(_) => continue,
        }
    }
    let RedirectOutcome::Blocked(status) = last else {
        unreachable!("Target und Gone kehren in der Schleife zurück");
    };
    bail!(
        "Kein Redirect-Ziel für {url} nach {} Versuchen (zuletzt HTTP {status}) — \
         die Seite antwortet, aber ohne Location; von einem Mac beantwortet dieselbe \
         URL sie sofort mit 308, das riecht nach einer Blockade der Runner-IP",
        backoff_ms.len() + 1
    )
}

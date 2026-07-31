//! Spiegelt Produktbilder in den öffentlichen Supabase-Storage-Bucket
//! `offer-images`. Die iOS-App bekommt dadurch stabile URLs statt der
//! rotierenden oder hotlink-geschützten Händler-CDNs.
//!
//! Idempotent & content-adressiert: Der Objektpfad leitet sich aus einem
//! sha256 der Quell-URL ab, der Upload nutzt `x-upsert`. Gleiches Bild ->
//! gleicher Pfad -> kein Duplikat. Den teuren Download sparen sich Aufrufer
//! über den lokalen SQLite-Cache (`db::cached_image_url`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::push::PushConfig;

pub const BUCKET: &str = "offer-images";

/// Erlaubte Bild-Hosts der Händler-CDNs (SSRF-Schutz). Abgeleitet aus den
/// Bild-URLs, die die Scraper tatsächlich erzeugen — belegt durch die Fixtures
/// unter `tests/fixtures/` und die fest kodierten URLs der Scraper, NICHT aus
/// beliebigem Fremd-Input:
///   penny.de          — penny.rs      (imageRendition -> `cdn.penny.de`)
///   netto-online.de   — netto.rs      (img.tc-product-image -> `www.netto-online.de`)
///   rewe-static.de    — rewe.rs       (images[] -> `img.rewe-static.de`)
///   media.schwarz     — kaufland.rs   (img.k-product-tile -> `kaufland.media.schwarz`)
///   aldi.cx           — aldi_sued.rs  (assets[].url -> `dm.emea.cms.aldi.cx`)
///   s7g10.scene7.com  — aldi_nord.rs  (assets[].url -> Adobe-Scene7-Shard)
///   edeka             — edeka.rs      (img src -> `offer-images.api.edeka`; die
///                                      `.edeka`-gTLD gehört komplett EDEKA)
/// Ein Host passt bei exakter Gleichheit oder als echte Subdomain eines
/// Eintrags (host == suffix || host endet auf ".{suffix}"). Neue Händler-CDNs
/// hier ergänzen.
const ALLOWED_IMAGE_HOST_SUFFIXES: &[&str] = &[
    "penny.de",
    "netto-online.de",
    "rewe-static.de",
    "media.schwarz",
    "aldi.cx",
    "s7g10.scene7.com",
    "edeka",
];

/// Bild-Hosts, die **jeden** schlichten HTTP-Client mit 403 abweisen — auch
/// die App. Gemessen am 2026-07-31 gegen `www.netto-online.de/media_nfs/…`:
///
/// * iPhone-UA + `Referer: netto-online.de` → 403 (kein Hotlink-Schutz),
/// * curl mit Chrome-UA allein → 403,
/// * reqwest mit **komplettem** Browser-Header-Satz → 403 (Akamai
///   fingerprintet den TLS-Stack, wie schon beim Scraper, siehe
///   `scrapers::util`),
/// * System-curl mit UA + Accept-Language + Accept-Encoding + Sec-Fetch-Trio
///   → 200. Jeder dieser Header ist notwendig; einer allein reicht nicht.
///
/// Für diese Hosts gilt beides zugleich: Die App kann das Bild nie laden
/// (Spiegeln ist Pflicht, nicht Option), und der Download beim Spiegeln muss
/// über System-curl laufen statt über reqwest.
const CLIENT_BLOCKED_HOST_SUFFIXES: &[&str] = &["netto-online.de"];

/// True, wenn die URL auf einen Host zeigt, dessen CDN schlichte Clients
/// (App eingeschlossen) mit 403 abweist — siehe
/// [`CLIENT_BLOCKED_HOST_SUFFIXES`]. `push::mirror_images` spiegelt solche
/// Bilder immer, unabhängig vom `mirror_images`-Schalter.
pub fn is_client_blocked_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|host| {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            CLIENT_BLOCKED_HOST_SUFFIXES
                .iter()
                .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        })
}

/// Deterministischer Objektpfad: sha256(Quell-URL) + Datei-Endung der Quelle.
pub fn object_path(source_url: &str) -> String {
    let hash = Sha256::digest(source_url.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("{hex}.{}", extension(source_url))
}

/// Datei-Endung aus der URL (ohne Query/Fragment), auf bekannte Bildformate
/// beschränkt; Fallback "jpg".
fn extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.'))
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif" => ext,
        _ => "jpg".to_string(),
    }
}

/// Öffentliche Bucket-URL für einen Objektpfad.
pub fn public_url(base_url: &str, path: &str) -> String {
    format!("{base_url}/storage/v1/object/public/{BUCKET}/{path}")
}

/// SSRF-Schutz für eine Händler-Bild-URL, bevor sie geladen wird: nur https,
/// Host auf der CDN-Allow-Liste (keine IP-Literale, keine internen Hostnamen),
/// und keine Auflösung in private/lokale Adressbereiche. Fehler bedeutet
/// „nicht spiegeln“ — der Aufrufer (`push::mirror_images`) überspringt das
/// einzelne Bild und bricht den Push nicht ab.
fn validate_source_url(source_url: &str) -> Result<()> {
    let url = reqwest::Url::parse(source_url)
        .with_context(|| format!("Bild-URL nicht parsebar: {source_url}"))?;
    if url.scheme() != "https" {
        bail!("Bild-URL abgelehnt (kein https): {source_url}");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Bild-URL ohne Host: {source_url}"))?;

    // IP-Literale nie erlauben — echte Händler-CDNs tragen Domainnamen. Das
    // sperrt u. a. `https://192.168.1.1/…` und `https://169.254.169.254/…`.
    if host.parse::<IpAddr>().is_ok() {
        bail!("Bild-URL abgelehnt (IP-Literal als Host): {source_url}");
    }
    if !host_is_allowed(host) {
        bail!("Bild-URL abgelehnt (Host nicht auf CDN-Allow-Liste): {source_url}");
    }

    // Defense-in-depth: den (allow-gelisteten) Host auflösen und jede Ziel-IP
    // gegen private/lokale Bereiche prüfen, damit ein CDN-Name, der auf ein
    // internes Netz zeigt, nicht geladen wird.
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("Bild-Host nicht auflösbar: {host}"))?
        .collect();
    if addrs.is_empty() {
        bail!("Bild-Host ohne Adressen: {host}");
    }
    if let Some(bad) = addrs.iter().map(|a| a.ip()).find(|ip| is_blocked_ip(*ip)) {
        bail!("Bild-URL abgelehnt (Host löst in internen Bereich {bad} auf): {source_url}");
    }
    Ok(())
}

/// Host gegen die CDN-Allow-Liste prüfen: exakte Gleichheit oder echte
/// Subdomain eines Eintrags. Der führende Punkt in ".{suffix}" verhindert
/// Trick-Hosts wie `penny.de.attacker.tld` oder `evilpenny.de`.
fn host_is_allowed(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ALLOWED_IMAGE_HOST_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// Content-Type gegen eine Bild-Allow-Liste prüfen und den kanonischen Wert
/// zurückgeben. Alles andere -> None (Antwort wird NICHT hochgeladen, statt den
/// Remote-Header blind zu übernehmen).
fn allowed_image_content_type(content_type: &str) -> Option<&'static str> {
    let main = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match main.as_str() {
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/webp" => Some("image/webp"),
        "image/gif" => Some("image/gif"),
        "image/avif" => Some("image/avif"),
        _ => None,
    }
}

/// True, wenn die IP in einem privaten/lokalen Bereich liegt, der von außen
/// nicht erreichbar sein darf (Loopback, RFC1918, Link-Local inkl.
/// Cloud-Metadata 169.254.169.254, Unique-Local, unspecified …).
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local() // 169.254/16 (u. a. Cloud-Metadata 169.254.169.254)
        || ip.is_unspecified() // 0.0.0.0
        || ip.is_broadcast() // 255.255.255.255
        || ip.is_documentation()
        // Carrier-Grade-NAT 100.64.0.0/10
        || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64)
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    // IPv4-gemappte Adressen (::ffff:a.b.c.d) über die v4-Regeln prüfen.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4);
    }
    let seg0 = ip.segments()[0];
    // Unique-Local fc00::/7 und Link-Local fe80::/10.
    (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80
}

/// Maximale Kantenlänge (px) eines gespiegelten Bildes. Größere Bilder werden
/// unter Erhalt des Seitenverhältnisses hierauf verkleinert. ~1200 px reicht
/// für die Produkt-Kacheln der App und eine Vollbild-Detailansicht auch auf
/// aktuellen iPhones (Retina), spart aber gegenüber Voll-Auflösungs-CDN-Bildern
/// ein Vielfaches an Bucket-Speicher.
const MAX_IMAGE_EDGE: u32 = 1200;
/// JPEG-Qualität beim Re-Encoden verkleinerter JPEGs (0–100).
const JPEG_QUALITY: u8 = 82;
/// WebP-Qualität beim Re-Encoden von PNGs (0.0–100.0, lossy).
const WEBP_QUALITY: f32 = 80.0;

/// Verkleinert/re-encodet ein Bild vor dem Bucket-Upload, damit der
/// `offer-images`-Bucket unter dem Storage-Free-Limit bleibt (die CDN-Bilder
/// kommen teils in Voll-Auflösung: JPEGs bis ~1 MB, Produkt-Freisteller als
/// 1200×1200-**PNGs** mit bis zu ~2.6 MB).
///
/// - **JPEG**: nur wenn überdimensioniert → auf `MAX_IMAGE_EDGE` px kappen und
///   mit `JPEG_QUALITY` neu encoden; sonst `None` (JPEG ist schon kompakt).
/// - **PNG**: immer → **WebP lossy** (`WEBP_QUALITY`), vorher bei Bedarf kappen.
///   PNG komprimiert Fotos praktisch nicht (die Freisteller wiegen ~2 MB); WebP
///   drückt sie ~90 %, **erhält aber die Transparenz** — anders als JPEG, das
///   den Alpha-Kanal auf eine harte Hintergrundfarbe plätten würde.
///
/// Der content-adressierte Objektpfad (`object_path`) hängt an der **Quell**-
/// Endung und bleibt dadurch stabil, auch wenn wir PNG→WebP re-encoden: ein
/// erneuter Sync überschreibt via `x-upsert` denselben Pfad (kein Orphan-Churn),
/// der zurückgegebene Content-Type (`image/webp`) sorgt fürs korrekte Ausliefern
/// — iOS/Browser dekodieren nach Inhalt, nicht nach Datei-Endung.
///
/// Liefert `None` (→ Original unverändert hochladen) bei bereits kleinen JPEGs,
/// anderen Formaten (webp/gif/avif — bereits kompakt bzw. selten) oder wenn
/// Dekodierung/Enkodierung fehlschlägt. Ein Downscaling-Fehler darf den Push
/// nie abbrechen.
fn downscale(bytes: &[u8], content_type: &str) -> Option<(Vec<u8>, &'static str)> {
    match content_type {
        "image/jpeg" => {
            let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?;
            if img.width() <= MAX_IMAGE_EDGE && img.height() <= MAX_IMAGE_EDGE {
                return None; // schon innerhalb des Limits — Original unangetastet
            }
            let resized = img.resize(
                MAX_IMAGE_EDGE,
                MAX_IMAGE_EDGE,
                image::imageops::FilterType::Lanczos3,
            );
            let mut out = std::io::Cursor::new(Vec::new());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
                .encode_image(&resized)
                .ok()?;
            Some((out.into_inner(), "image/jpeg"))
        }
        "image/png" => {
            let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
            // Nur kappen, wenn überdimensioniert; resize() erhält das Seitenverhältnis.
            let img = if img.width() > MAX_IMAGE_EDGE || img.height() > MAX_IMAGE_EDGE {
                img.resize(
                    MAX_IMAGE_EDGE,
                    MAX_IMAGE_EDGE,
                    image::imageops::FilterType::Lanczos3,
                )
            } else {
                img
            };
            // WebP lossy; Alpha wird von libwebp erhalten (verlustfrei kodiert).
            let encoded = webp::Encoder::from_image(&img).ok()?.encode(WEBP_QUALITY);
            Some((encoded.to_vec(), "image/webp"))
        }
        _ => None,
    }
}

/// Bild vom Händler laden und in den Bucket hochladen; liefert die öffentliche
/// Bucket-URL. Idempotent dank content-adressiertem Pfad + `x-upsert` — ein
/// erneuter Aufruf für dasselbe Bild überschreibt dasselbe Objekt.
pub fn mirror(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    source_url: &str,
) -> Result<String> {
    // Kachelbilder aus dem Lidl-Prospekt liegen als Datei vor, nicht hinter
    // einer URL — sie werden lokal aus dem PDF geschnitten. Der Weg dahin ist
    // derselbe wie für jedes Händlerbild (downscale -> WebP -> Bucket), nur die
    // Quelle ist eine andere.
    let local = local_crop_path(source_url)?;

    // SSRF-Schutz: Quelle prüfen, BEVOR irgendein Netzwerkzugriff erfolgt.
    // Für eine lokale Datei gibt es keinen Netzwerkzugriff und keinen Host,
    // dafür die Schranke in `local_crop_path`.
    if local.is_none() {
        validate_source_url(source_url)?;
    }

    let path = object_path(source_url);

    if let Some(file) = local {
        let bytes = std::fs::read(&file)
            .with_context(|| format!("Kachelbild nicht lesbar: {}", file.display()))?;
        // Immer PNG: so schneidet `pdftoppm`. `downscale` macht daraus WebP.
        let (body, content_type) = match downscale(&bytes, "image/png") {
            Some((scaled, ct)) => (scaled, ct),
            None => (bytes, "image/png"),
        };
        return upload(client, cfg, &path, body, content_type);
    }

    // Bild vom Händler-CDN laden. Beide Wege folgen bewusst keinen Redirects,
    // damit ein 3xx nicht auf ein internes Ziel umgelenkt werden kann:
    // `client` ist entsprechend gebaut (siehe `push::run`), und `curl_download`
    // ruft curl ohne `-L` auf.
    let (bytes, remote_ct) = if is_client_blocked_url(source_url) {
        // Akamai blockt den reqwest-TLS-Stack (403, gemessen 2026-07-31 mit
        // identischen Headern) — derselbe Befund wie beim Netto-Scraper.
        // System-curl mit vollem Browser-Header-Satz kommt durch.
        curl_download(source_url)?
    } else {
        let resp = client
            .get(source_url)
            .send()
            .with_context(|| format!("Bild-Download fehlgeschlagen: {source_url}"))?;
        if !resp.status().is_success() {
            bail!("Bild-Download {source_url}: HTTP {}", resp.status());
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = resp.bytes().context("Bild-Bytes lesen fehlgeschlagen")?.to_vec();
        (bytes, ct)
    };
    // Content-Type gegen die Bild-Allow-Liste prüfen statt den Remote-Header
    // blind weiterzureichen; Nicht-Bilder werden nicht hochgeladen.
    let Some(content_type) = allowed_image_content_type(&remote_ct) else {
        bail!("Bild-Download {source_url}: unerlaubter Content-Type '{remote_ct}'");
    };

    // Überdimensionierte Bilder vor dem Upload verkleinern, damit der Bucket
    // unter dem Storage-Free-Limit bleibt. Bei zu kleinen Bildern, anderen
    // Formaten oder Fehlern bleibt das Original (und sein Content-Type).
    let (body, content_type): (Vec<u8>, &str) = match downscale(&bytes, content_type) {
        Some((scaled, ct)) => (scaled, ct),
        None => (bytes, content_type),
    };

    upload(client, cfg, &path, body, content_type)
}

/// Bild-Download über System-curl mit vollem Browser-Header-Satz — für Hosts,
/// deren Akamai-Schutz den reqwest-TLS-Stack blockt (siehe
/// [`CLIENT_BLOCKED_HOST_SUFFIXES`]). Liefert (Bytes, Content-Type).
///
/// Bewusst **ohne** `-L`: Redirects nicht folgen, dieselbe SSRF-Schranke wie
/// beim reqwest-Client in `push::run`. Der Body landet in einer Temp-Datei
/// statt auf stdout, damit sich Binärdaten nicht mit dem `-w`-Statusreport
/// mischen.
fn curl_download(url: &str) -> Result<(Vec<u8>, String)> {
    // Präzedenzfall `scrapers::util::curl_get` — inklusive der höflichen
    // Pause zwischen Requests an denselben Host.
    crate::scrapers::util::polite_pause(url);
    let tmp = std::env::temp_dir().join(format!(
        "lechariot-bild-{}-{}.part",
        std::process::id(),
        object_path(url)
    ));
    let output = std::process::Command::new("curl")
        .arg("-s")
        .arg("--max-time")
        .arg("30")
        .arg("-o")
        .arg(&tmp)
        .arg("-w")
        .arg("%{http_code}\t%{content_type}")
        // Der volle Satz ist notwendig: UA, Accept-Language, Accept-Encoding
        // UND das Sec-Fetch-Trio — fehlt einer, antwortet Akamai mit 403
        // (einzeln durchgemessen am 2026-07-31).
        .args(["-H", &format!("User-Agent: {}", crate::scrapers::util::USER_AGENT)])
        .args(["-H", "Accept-Language: de-DE,de;q=0.9,en;q=0.8"])
        .args(["-H", "Accept-Encoding: gzip, deflate, br"])
        .arg("--compressed")
        .args(["-H", "Accept: image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8"])
        .args(["-H", "Sec-Fetch-Site: same-origin"])
        .args(["-H", "Sec-Fetch-Mode: no-cors"])
        .args(["-H", "Sec-Fetch-Dest: image"])
        .arg(url)
        .output()
        .context("curl nicht gefunden — wird fürs Spiegeln von Netto-Bildern benötigt")?;

    let report = String::from_utf8_lossy(&output.stdout);
    let (status, content_type) = report.trim().split_once('\t').unwrap_or((report.trim(), ""));
    let bytes = std::fs::read(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    if status != "200" {
        bail!("Bild-Download {url}: HTTP {status} (curl)");
    }
    if bytes.is_empty() {
        bail!("Bild-Download {url}: leerer Body (curl)");
    }
    Ok((bytes, content_type.to_string()))
}

/// `file://`-Quelle in einen Pfad auflösen, aber **nur** innerhalb des
/// Kachelbild-Verzeichnisses.
///
/// Die Schranke ist der Ersatz für die Host-Allow-Liste: Ohne sie würde aus
/// einer Bild-URL ein Leseprimitiv auf das ganze Dateisystem, und der Inhalt
/// landete öffentlich im Bucket. Erlaubt ist deshalb genau das Verzeichnis,
/// in das `lidl_prospekt` schneidet — und keine Pfadwechsel darin.
fn local_crop_path(source_url: &str) -> Result<Option<std::path::PathBuf>> {
    let Some(rest) = source_url.strip_prefix("file://") else {
        return Ok(None);
    };
    let path = std::path::Path::new(rest);
    let dir = crate::scrapers::lidl_prospekt::crop_dir();
    if path.components().any(|c| c == std::path::Component::ParentDir) {
        bail!("Kachelbild mit Pfadwechsel abgelehnt: {source_url}");
    }
    if !path.starts_with(&dir) {
        bail!(
            "Kachelbild außerhalb von {} abgelehnt: {source_url}",
            dir.display()
        );
    }
    Ok(Some(path.to_path_buf()))
}

/// In den Bucket hochladen (idempotent via x-upsert).
fn upload(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    path: &str,
    body: Vec<u8>,
    content_type: &str,
) -> Result<String> {
    let upload_url = format!("{}/storage/v1/object/{BUCKET}/{path}", cfg.base_url);
    let resp = client
        .post(&upload_url)
        .header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .header("Content-Type", content_type)
        .header("x-upsert", "true")
        .body(body)
        .send()
        .with_context(|| format!("Bild-Upload fehlgeschlagen: {path}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        let excerpt: String = body.chars().take(200).collect();
        bail!("Bild-Upload {path}: HTTP {status}: {excerpt}");
    }

    Ok(public_url(&cfg.base_url, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Aus einer Bild-URL darf kein Leseprimitiv auf das Dateisystem werden.
    // `file://` gibt es nur für die Kachelbilder, die dieser Lauf selbst aus
    // dem Prospekt-PDF geschnitten hat — alles andere fliegt raus, bevor
    // irgendetwas gelesen und öffentlich in den Bucket gelegt wird.
    #[test]
    fn local_crops_are_confined_to_their_directory() {
        let dir = crate::scrapers::lidl_prospekt::crop_dir();
        let ok = format!("file://{}/LIDL_1988_abc.png", dir.display());
        assert_eq!(
            local_crop_path(&ok).unwrap(),
            Some(dir.join("LIDL_1988_abc.png"))
        );

        // Woanders im Dateisystem: abgelehnt.
        assert!(local_crop_path("file:///etc/passwd").is_err());
        assert!(local_crop_path("file:///tmp/fremd/bild.png").is_err());

        // Pfadwechsel aus dem erlaubten Verzeichnis heraus: ebenfalls.
        let escape = format!("file://{}/../../etc/passwd", dir.display());
        assert!(local_crop_path(&escape).is_err());

        // http(s) bleibt unberührt und geht den normalen Weg.
        assert_eq!(
            local_crop_path("https://img.rewe-static.de/x.jpg").unwrap(),
            None
        );
    }

    // Netto-URLs (und nur sie) gelten als client-blockiert: Das CDN weist
    // jeden schlichten Client mit 403 ab, die App eingeschlossen. Diese
    // Zuordnung entscheidet in push::scope_of, ob ein Bild Pflicht ist.
    #[test]
    fn client_blocked_hosts_are_exactly_the_netto_cdn() {
        assert!(is_client_blocked_url(
            "https://www.netto-online.de/media_nfs/images/2026-31/42-450x450-n.webp"
        ));
        assert!(is_client_blocked_url("https://netto-online.de/bild.webp"));
        // Alle anderen Ketten liefern an die App aus (gemessen 2026-07-31,
        // iPhone-UA: REWE/EDEKA/Kaufland/Penny/beide ALDIs -> 200).
        for url in [
            "https://img.rewe-static.de/x.jpg",
            "https://cdn.penny.de/x.jpg",
            "https://kaufland.media.schwarz/x.png",
            "https://dm.emea.cms.aldi.cx/x.jpg",
            "https://s7g10.scene7.com/x.jpg",
            "https://offer-images.api.edeka/x.jpg",
        ] {
            assert!(!is_client_blocked_url(url), "fälschlich blockiert: {url}");
        }
        // Trick-Hosts zählen nicht als Netto.
        assert!(!is_client_blocked_url("https://netto-online.de.attacker.tld/x.jpg"));
        assert!(!is_client_blocked_url("file:///tmp/x.png"));
        assert!(!is_client_blocked_url("kaputt"));
    }

    /// Lokaler HTTP-Server mit einer festen Antwort, für die curl-Downloads.
    fn spawn_static_server(status: &'static str, headers: &'static str, body: &'static [u8]) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 {status}\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://{addr}")
    }

    // Der curl-Weg muss Binärdaten unversehrt liefern und den Content-Type
    // mitbringen — er ersetzt reqwest für Hosts, deren Akamai-Schutz den
    // reqwest-TLS-Stack blockt (gemessen 2026-07-31: 403 trotz identischer
    // Header).
    #[test]
    fn curl_download_returns_bytes_and_content_type() {
        let body: &[u8] = &[0x52, 0x49, 0x46, 0x46, 0x00, 0xFF, 0x10, 0x20];
        let base = spawn_static_server("200 OK", "content-type: image/webp\r\n", body);
        let (bytes, ct) = curl_download(&format!("{base}/bild.webp")).unwrap();
        assert_eq!(bytes, body, "Binärdaten müssen unversehrt ankommen");
        assert_eq!(ct, "image/webp");
    }

    #[test]
    fn curl_download_fails_on_non_200() {
        let base = spawn_static_server("403 Forbidden", "content-type: text/html\r\n", b"Access Denied");
        let err = curl_download(&format!("{base}/bild.webp")).unwrap_err().to_string();
        assert!(err.contains("403"), "{err}");
    }

    #[test]
    fn object_path_is_deterministic() {
        let url = "https://cdn.example/wassermelone.png";
        assert_eq!(object_path(url), object_path(url));
        // andere URL -> anderer Pfad
        assert_ne!(object_path(url), object_path("https://cdn.example/gouda.png"));
    }

    #[test]
    fn object_path_keeps_known_extension() {
        assert!(object_path("https://x/a.png").ends_with(".png"));
        assert!(object_path("https://x/a.jpeg").ends_with(".jpeg"));
        // Query wird ignoriert
        assert!(object_path("https://x/a.webp?w=450").ends_with(".webp"));
    }

    #[test]
    fn object_path_falls_back_to_jpg() {
        assert!(object_path("https://x/image").ends_with(".jpg"));
        assert!(object_path("https://x/a.bin").ends_with(".jpg"));
    }

    #[test]
    fn public_url_points_at_bucket() {
        assert_eq!(
            public_url("https://p.supabase.co", "abc.png"),
            "https://p.supabase.co/storage/v1/object/public/offer-images/abc.png"
        );
    }

    // ----------------------------------------------------- SSRF-Validierung

    #[test]
    fn host_allow_list_accepts_real_retailer_hosts() {
        // Exakt die Hosts, die die Scraper laut Fixtures erzeugen.
        for host in [
            "cdn.penny.de",
            "www.netto-online.de",
            "img.rewe-static.de",
            "kaufland.media.schwarz",
            "dm.emea.cms.aldi.cx",
            "s7g10.scene7.com",
                    "offer-images.api.edeka",
        ] {
            assert!(host_is_allowed(host), "sollte erlaubt sein: {host}");
        }
    }

    #[test]
    fn host_allow_list_rejects_lookalikes_and_foreign_hosts() {
        for host in [
            "evil.example.com",
            "penny.de.attacker.tld", // Suffix-Trick
            "evilpenny.de",          // kein Punkt vor penny.de
            "notpenny.de",
            "metadata.google.internal",
            "localhost",
        ] {
            assert!(!host_is_allowed(host), "sollte abgelehnt werden: {host}");
        }
    }

    #[test]
    fn validate_rejects_non_https_and_ip_literals_and_foreign_hosts() {
        // http statt https.
        assert!(validate_source_url("http://cdn.penny.de/a.jpg").is_err());
        // IP-Literale (inkl. Cloud-Metadata) — vor jeder DNS-Auflösung.
        assert!(validate_source_url("https://192.168.1.1/status").is_err());
        assert!(validate_source_url("https://127.0.0.1/a.jpg").is_err());
        assert!(validate_source_url("https://169.254.169.254/latest/meta-data").is_err());
        // Fremder Host, nicht auf der Allow-Liste.
        assert!(validate_source_url("https://evil.example.com/a.jpg").is_err());
        // Kaputte URL.
        assert!(validate_source_url("not a url").is_err());
    }

    #[test]
    fn content_type_allow_list() {
        assert_eq!(allowed_image_content_type("image/jpeg"), Some("image/jpeg"));
        assert_eq!(allowed_image_content_type("IMAGE/JPEG"), Some("image/jpeg"));
        assert_eq!(allowed_image_content_type("image/jpg"), Some("image/jpeg"));
        assert_eq!(allowed_image_content_type("image/png; charset=binary"), Some("image/png"));
        assert_eq!(allowed_image_content_type("image/webp"), Some("image/webp"));
        assert_eq!(allowed_image_content_type("image/avif"), Some("image/avif"));
        // Nicht-Bilder werden abgelehnt.
        assert_eq!(allowed_image_content_type("application/json"), None);
        assert_eq!(allowed_image_content_type("text/html"), None);
        assert_eq!(allowed_image_content_type(""), None);
    }

    // --------------------------------------------------------- Downscaling

    /// Erzeugt ein einfarbiges Bild der gegebenen Größe als encodete Bytes.
    fn encode_test_image(w: u32, h: u32, format: image::ImageFormat) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(w, h));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    /// Verrauschtes RGBA-Bild (schlecht komprimierbar, etwas Transparenz), als
    /// PNG encodet — so ist das PNG groß genug, dass WebP messbar gewinnt.
    fn noisy_png(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for (x, y, px) in img.enumerate_pixels_mut() {
            let r = ((x * 7 + y * 13) % 256) as u8;
            let g = ((x * 3 + y * 5) % 256) as u8;
            let b = ((x ^ y) % 256) as u8;
            let a = if (x + y) % 9 == 0 { 0 } else { 255 }; // echte Transparenz
            *px = image::Rgba([r, g, b, a]);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    /// WebP-Signatur: `RIFF`…`WEBP`.
    fn is_webp(bytes: &[u8]) -> bool {
        bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
    }

    #[test]
    fn downscale_shrinks_oversized_jpeg() {
        let bytes = encode_test_image(2000, 1500, image::ImageFormat::Jpeg);
        let (scaled, ct) = downscale(&bytes, "image/jpeg").expect("sollte verkleinern");
        assert_eq!(ct, "image/jpeg");
        let img = image::load_from_memory(&scaled).unwrap();
        assert!(img.width() <= MAX_IMAGE_EDGE && img.height() <= MAX_IMAGE_EDGE);
        // Seitenverhältnis bleibt erhalten (Breite ist die längere Kante).
        assert_eq!(img.width(), MAX_IMAGE_EDGE);
    }

    #[test]
    fn downscale_png_becomes_smaller_webp() {
        // Großes PNG -> WebP: als WebP ausgegeben und deutlich kleiner.
        let png = noisy_png(1600, 1600);
        let (out, ct) = downscale(&png, "image/png").expect("PNG sollte zu WebP werden");
        assert_eq!(ct, "image/webp");
        assert!(is_webp(&out), "Output sollte WebP-Signatur haben");
        assert!(
            out.len() < png.len(),
            "WebP ({}) sollte kleiner als PNG ({}) sein",
            out.len(),
            png.len()
        );
    }

    #[test]
    fn downscale_small_png_still_converts_to_webp() {
        // Auch PNGs innerhalb des Limits werden zu WebP (Alpha bleibt, kompakter).
        let png = noisy_png(400, 300);
        let (out, ct) = downscale(&png, "image/png").expect("auch kleines PNG -> WebP");
        assert_eq!(ct, "image/webp");
        assert!(is_webp(&out));
    }

    #[test]
    fn downscale_leaves_small_jpeg_and_foreign_formats_untouched() {
        // JPEG innerhalb des Limits -> None (Original unverändert hochladen).
        let small = encode_test_image(400, 300, image::ImageFormat::Jpeg);
        assert!(downscale(&small, "image/jpeg").is_none());
        // Andere Formate werden nicht angefasst.
        assert!(downscale(&small, "image/webp").is_none());
        assert!(downscale(b"nonsense", "image/gif").is_none());
        // Kaputte Bytes -> None statt Panik (kein Push-Abbruch).
        assert!(downscale(b"not an image", "image/jpeg").is_none());
        assert!(downscale(b"not a png", "image/png").is_none());
    }

    #[test]
    fn blocked_ip_covers_private_and_local_ranges() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254", // Cloud-Metadata (Link-Local)
            "100.64.0.1",      // Carrier-Grade-NAT
            "0.0.0.0",
            "::1",
            "::",
            "fc00::1",           // Unique-Local
            "fe80::1",           // Link-Local
            "::ffff:127.0.0.1",  // IPv4-gemappt
        ] {
            assert!(is_blocked_ip(ip.parse().unwrap()), "sollte blockiert sein: {ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(!is_blocked_ip(ip.parse().unwrap()), "sollte erlaubt sein: {ip}");
        }
    }
}

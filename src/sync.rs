use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::branches;
use crate::db;
use crate::models::{Branch, Market, Offer};
use crate::push::{self, PushConfig, PushOptions, SupabaseRow};
use crate::stores::save_offers;

/// Ketten mit bundesweit identischem Angebotskatalog: deren Angebote können
/// beim On-Demand-Sync aus einer bereits gesyncten Region kopiert werden,
/// bevor irgendein Scraper läuft.
pub const NATIONAL_CHAINS: [&str; 3] = ["Lidl", "ALDI Nord", "ALDI SÜD"];

/// Filial-Lookup für nationale Ketten: (Ketten-Anzeigename, PLZ) ->
/// Ok(Some(Filiale)), Ok(None) = keine Filiale im Umkreis, Err = Finder kaputt.
/// In Produktion die find_market-Funktionen der Scraper; in Tests ein Stub.
pub type BranchFinder<'a> = dyn Fn(&str, &str) -> Result<Option<Market>> + 'a;

/// Ergebnis des Scrapens einer Region: pro Kette (Anzeigename wie in
/// `Store::chain()`) Markt + Angebote, `Ok(None)` wenn die Kette laut
/// Store-Finder keine Filiale im Umkreis der PLZ hat, oder ein Fehler.
pub type FetchResult = Vec<(String, Result<Option<(Market, Vec<Offer>)>>)>;

/// Scrape-Funktion, die für eine PLZ alle Ketten abruft. In Produktion eine
/// Closure über `Store::ALL` + `scrape_store`; in Tests ein Stub ohne Netz.
pub type Fetcher<'a> = dyn Fn(&str) -> FetchResult + 'a;

pub struct SyncOptions {
    pub db_path: String,
    pub dry_run: bool,
    /// Höchstens so viele Regionen pro Lauf syncen; weitere werden geloggt
    /// und übersprungen.
    pub max_regions: usize,
    /// Nur diese eine PLZ syncen (On-Demand-Trigger); wird bei Bedarf in
    /// `regions` registriert. None = alle aktiven Regionen.
    pub only: Option<String>,
    /// Nur diese eine Filiale syncen (`run_branch`). Schließt `only` aus.
    pub market_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Region {
    pub plz: String,
}

fn check_response(what: &str, resp: reqwest::blocking::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().unwrap_or_default();
    let excerpt: String = body.chars().take(300).collect();
    bail!("{what} fehlgeschlagen (HTTP {status}): {excerpt}");
}

fn auth(
    cfg: &PushConfig,
    req: reqwest::blocking::RequestBuilder,
) -> reqwest::blocking::RequestBuilder {
    req.header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
}

/// Aktive Regionen aus Supabase laden, älteste Anfrage zuerst.
pub fn fetch_regions(cfg: &PushConfig) -> Result<Vec<Region>> {
    let client = reqwest::blocking::Client::new();
    let resp = auth(cfg, client.get(format!("{}/rest/v1/regions", cfg.base_url)))
        .query(&[
            ("select", "plz"),
            ("active", "eq.true"),
            // Unsyncte Regionen zuerst (dort wartet gerade jemand in der App),
            // danach die ältesten — so bekommt eine neue PLZ Daten in Minuten.
            ("order", "last_synced.asc.nullsfirst,requested_at.asc"),
        ])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let excerpt: String = body.chars().take(300).collect();
        bail!("Regionen laden fehlgeschlagen (HTTP {status}): {excerpt}");
    }
    let regions: Vec<Region> = resp.json().context("Regionen-Antwort ist kein gültiges JSON")?;
    Ok(regions)
}

/// Markt-Zeile einer Kette für eine Region aus `public.markets` löschen —
/// der Store-Finder hat definitiv "keine Filiale im Umkreis" gemeldet.
/// Finder-Fehler kommen hier nie an (sie fallen in store_finder::resolve auf
/// den nationalen Platzhalter zurück), die Zeile bleibt dann stehen.
fn delete_market(cfg: &PushConfig, chain: &str, plz: &str) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let resp = auth(cfg, client.delete(format!("{}/rest/v1/markets", cfg.base_url)))
        .query(&[("chain", format!("eq.{chain}")), ("plz", format!("eq.{plz}"))])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    check_response(&format!("Löschen des Markts {chain} (PLZ {plz})"), resp)
}

/// Angebote einer Kette für eine Region aus `public.offers` löschen — Gegenstück
/// zu `delete_market`. Ohne Filiale in der Region hat die Kette dort auch keine
/// Angebote; der Push räumt sie nicht ab, weil er nur Wochen von Ketten löscht,
/// die er selbst pusht. Bleiben sie stehen, sind sie für immer verwaist: so
/// standen in Köln (50667) ALDI-SÜD-Angebote unter `market='ALDI Nord'`.
/// Ausgelöst wird das nur von einem *erfolgreichen* "keine Filiale"-Lookup —
/// Finder-Fehler kommen hier nie an.
fn delete_offers(cfg: &PushConfig, chain: &str, plz: &str) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let resp = auth(cfg, client.delete(format!("{}/rest/v1/offers", cfg.base_url)))
        .query(&[("market", format!("eq.{chain}")), ("region", format!("eq.{plz}"))])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    check_response(&format!("Löschen der Angebote von {chain} (Region {plz})"), resp)
}

/// Gefundene Märkte einer Region nach `public.markets` upserten.
fn upsert_markets(cfg: &PushConfig, rows: &[serde_json::Value]) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let resp = auth(cfg, client.post(format!("{}/rest/v1/markets", cfg.base_url)))
        .query(&[("on_conflict", "chain,plz")])
        .header("Prefer", "resolution=merge-duplicates")
        .json(rows)
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    check_response(&format!("Upsert von {} Märkten", rows.len()), resp)
}

/// Angebote der nationalen Ketten aus vorhandenen Supabase-Daten in eine neue
/// Region kopieren — noch bevor irgendein Scraper läuft. Pro Kette:
///
/// 1. Filialcheck VORAB über den Store-Finder (statt Kopieren + späterem
///    Cleanup): ohne Filiale gäbe es keine markets-Zeile und die kopierten
///    Angebote wären für die App unsichtbar. Aufräumen würde sie erst der
///    nächste Sync der Region (`delete_offers` in `sync_region`) — der Check
///    kostet ~2 Requests pro Kette und spart diesen Umweg.
///    Finder-Fehler überspringen die Kette nur (kein Kopieren, kein Abbruch) —
///    der reguläre Scrape direkt danach hat seinen eigenen Fallback.
/// 2. ALLE noch gültigen Wochen der Kette kopieren, nicht nur die neueste:
///    Lidl führt bis zu 6 überlappende Wochen parallel (die mit max.
///    valid_from ist die Non-Food-Vorschau des Onlineshops — nur sie zu
///    kopieren zeigte in der App 30 Onlineshop-Artikel statt ~255 Angebote).
///    Quellregion ist die mit den meisten aktuell gültigen Zeilen der Kette —
///    robuster als "zuletzt gesynct", denn eine gerade halb gescheiterte
///    Region wäre zwar die neueste, hätte aber Lücken. Kopiert wird per
///    Upsert mit dem Push-Konfliktschlüssel; der Scrape merged später drüber.
/// 3. Die gefundene Filiale nach `public.markets` melden, damit die App die
///    Kette sofort anzeigt.
///
/// Der Seed ist reine Sofort-Anzeige, KEIN Ersatz für den Scrape: die Kette
/// wird im selben Lauf trotzdem normal gescrapet, und deren Push überschreibt
/// idempotent (alte Wochen löschen + vollen Katalog upserten). Das ist
/// robuster, als den Scrape für geseedete Ketten zu überspringen — eine
/// unvollständige oder veraltete Kopie heilt sich so im selben Lauf selbst.
///
/// Liefert die Zahl kopierter Angebote; Fehler einzelner Ketten warnen nur.
pub fn seed_national_chains(cfg: &PushConfig, finder: &BranchFinder, plz: &str) -> usize {
    let mut total = 0usize;
    for chain in NATIONAL_CHAINS {
        match seed_chain(cfg, finder, chain, plz) {
            Ok(n) => {
                if n > 0 {
                    println!("[{plz}] {chain}: {n} Angebote aus vorhandener Region kopiert.");
                }
                total += n;
            }
            Err(e) => eprintln!("WARNUNG [{plz}] {chain}: Vorab-Kopie fehlgeschlagen: {e:#}"),
        }
    }
    total
}

fn seed_chain(cfg: &PushConfig, finder: &BranchFinder, chain: &str, plz: &str) -> Result<usize> {
    // Eigener, kurzlebiger Client wie in den übrigen Supabase-Helfern hier —
    // upsert_markets unten baut seine eigene Verbindung auf.
    let client = reqwest::blocking::Client::new();
    let market = match finder(chain, plz) {
        Ok(Some(m)) => m,
        Ok(None) => {
            println!("[{plz}] {chain}: keine Filiale in der Nähe — Vorab-Kopie übersprungen.");
            return Ok(0);
        }
        Err(e) => {
            eprintln!("WARNUNG [{plz}] {chain}: Filialsuche fehlgeschlagen ({e:#}) — Vorab-Kopie übersprungen.");
            return Ok(0);
        }
    };

    // Beste Quellregion finden: die mit den meisten aktuell gültigen Zeilen
    // der Kette (nur region-Spalte laden, client-seitig zählen — Aggregate
    // sind in PostgREST standardmäßig aus). Die Ziel-PLZ selbst zählt nicht.
    let offers_url = format!("{}/rest/v1/offers", cfg.base_url);
    let today = chrono::Utc::now()
        .with_timezone(&chrono_tz::Europe::Berlin)
        .format("%Y-%m-%d")
        .to_string();
    let resp = auth(cfg, client.get(&offers_url))
        .query(&[
            ("select", "region"),
            ("market", &format!("eq.{chain}")),
            ("valid_until", &format!("gte.{today}")),
        ])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    if !resp.status().is_success() {
        let status = resp.status();
        bail!("Quellregion-Suche fehlgeschlagen (HTTP {status})");
    }
    let probe: Vec<serde_json::Value> =
        resp.json().context("Quellregion-Suche: Antwort ist kein gültiges JSON")?;
    let mut counts: std::collections::HashMap<Option<String>, usize> =
        std::collections::HashMap::new();
    for hit in &probe {
        let region = hit.get("region").and_then(|v| v.as_str()).map(String::from);
        if region.as_deref() == Some(plz) {
            continue;
        }
        *counts.entry(region).or_default() += 1;
    }
    let Some((source_region, _)) = counts.into_iter().max_by_key(|(_, n)| *n) else {
        println!("[{plz}] {chain}: keine gültigen Angebote in anderen Regionen — nichts zu kopieren.");
        return Ok(0);
    };

    // Alle aktuell UND künftig gültigen Zeilen der Quellregion laden und mit
    // region=<PLZ> upserten — abgelaufene Wochen bleiben weg.
    let region_filter = match &source_region {
        Some(r) => ("region", format!("eq.{r}")),
        None => ("region", "is.null".to_string()),
    };
    let resp = auth(cfg, client.get(&offers_url))
        .query(&[
            ("market", &format!("eq.{chain}")),
            ("valid_until", &format!("gte.{today}")),
            (region_filter.0, &region_filter.1),
        ])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    if !resp.status().is_success() {
        let status = resp.status();
        bail!("Quell-Angebote laden fehlgeschlagen (HTTP {status})");
    }
    let mut rows: Vec<SupabaseRow> =
        resp.json().context("Quell-Angebote: Antwort passt nicht zum offers-Schema")?;
    // Auf die Ziel-Filiale umschreiben, nicht nur auf die Ziel-Region: Seit
    // migration_v13 ist `market_id` Teil des Angebots-Schlüssels. Bliebe die
    // Filiale der Quellregion stehen, käme der reguläre Scrape direkt danach
    // nie an diese Zeilen heran — er löscht und upsertet unter der Filiale,
    // die er selbst gefunden hat. Die Kopien lägen dann für immer verwaist
    // in der Zielregion.
    //
    // Damit trägt eine kopierte Zeile die Filiale, in der sie (noch) nicht
    // nachgewiesen ist. Das ist genau der Punkt, den Phase 11 unter
    // „Vorab-Kopie klären" entscheiden muss; hier wird er nur nicht
    // schlimmer gemacht als vorher.
    for row in &mut rows {
        row.region = Some(plz.to_string());
        row.market_id = market.id.clone();
    }
    if rows.is_empty() {
        return Ok(0);
    }
    push::upsert_offer_rows(&client, cfg, &rows, &format!("[{chain}] Vorab-Kopie"))?;
    drop(client);

    // Filiale melden, damit die App die Kette sofort anzeigt; der reguläre
    // Scrape upsertet dieselbe Zeile später einfach erneut.
    upsert_markets(
        cfg,
        &[serde_json::json!({
            "chain": chain,
            "branch_name": market.name,
            "market_id": market.id,
            "plz": plz,
            "lat": market.lat,
            "lon": market.lon,
            "updated_at": chrono::Utc::now().to_rfc3339(),
        })],
    )?;
    Ok(rows.len())
}

/// Eine Region komplett syncen: Ketten scrapen, Märkte upserten, Angebote
/// pushen. Die lokale offers-Tabelle wird vorher geleert, damit `push` nur
/// Angebote dieser Region hochlädt.
fn sync_region(
    opts: &SyncOptions,
    cfg: &PushConfig,
    fetcher: &Fetcher,
    plz: &str,
    defer_mirror: bool,
) -> Result<()> {
    {
        let conn = db::open(&opts.db_path)?;
        conn.execute("DELETE FROM offers", [])
            .context("Lokale offers-Tabelle konnte nicht geleert werden")?;
    }

    struct Row {
        chain: String,
        market: String,
        result: std::result::Result<usize, String>,
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut market_rows: Vec<serde_json::Value> = Vec::new();
    let mut gone_chains: Vec<String> = Vec::new();

    for (chain, result) in fetcher(plz) {
        match result {
            Ok(None) => {
                // Kette hat definitiv keine Filiale im Umkreis — nicht
                // registrieren (zählt weder als Erfolg noch als Fehler) und
                // eine evtl. vorhandene Markt-Zeile aus Supabase entfernen.
                println!("[{plz}] {chain}: keine Filiale in der Nähe — übersprungen.");
                gone_chains.push(chain);
            }
            Ok(Some((market, offers))) => {
                println!("[{plz}] {chain}: {} Angebote gefunden.", offers.len());
                if offers.is_empty() {
                    eprintln!(
                        "WARNUNG [{plz}] {chain}: Scraper lieferte 0 Angebote — Kette bleibt in dieser Region leer."
                    );
                }
                match save_offers(&opts.db_path, &market, &offers) {
                    Ok(()) => {
                        market_rows.push(serde_json::json!({
                            "chain": chain,
                            "branch_name": market.name,
                            "market_id": market.id,
                            "plz": plz,
                            "lat": market.lat,
                            "lon": market.lon,
                            "updated_at": chrono::Utc::now().to_rfc3339(),
                        }));
                        rows.push(Row { chain, market: market.name, result: Ok(offers.len()) });
                    }
                    Err(e) => {
                        eprintln!("[{plz}] {chain}: Speichern fehlgeschlagen: {e:#}");
                        rows.push(Row { chain, market: market.name, result: Err(format!("{e:#}")) });
                    }
                }
            }
            Err(e) => {
                eprintln!("[{plz}] {chain}: Fehler: {e:#}");
                rows.push(Row { chain, market: "-".to_string(), result: Err(format!("{e:#}")) });
            }
        }
    }

    println!("Zusammenfassung (Region {plz}):");
    println!("  {:<10} {:<28} {}", "Markt", "Filiale", "Ergebnis");
    for row in &rows {
        let result = match &row.result {
            Ok(n) => format!("{n} Angebote"),
            Err(e) => {
                let first = e.lines().next().unwrap_or("Fehler");
                format!("FEHLER: {first}")
            }
        };
        println!("  {:<10} {:<28} {}", row.chain, row.market, result);
    }

    if !rows.iter().any(|r| r.result.is_ok()) {
        bail!("Region {plz}: keine Kette erfolgreich abgerufen.");
    }

    if opts.dry_run {
        println!("[{plz}] Dry-Run — Märkte werden nicht hochgeladen.");
    } else {
        if !market_rows.is_empty() {
            upsert_markets(cfg, &market_rows)?;
            println!("[{plz}] {} Märkte nach Supabase gemeldet.", market_rows.len());
        }
        for chain in &gone_chains {
            delete_market(cfg, chain, plz)?;
            delete_offers(cfg, chain, plz)?;
            println!(
                "[{plz}] {chain}: Markt-Zeile und Angebote entfernt (keine Filiale mehr im Umkreis)."
            );
        }
    }

    push::run(
        &PushOptions {
            db_path: opts.db_path.clone(),
            chain: None,
            region: Some(plz.to_string()),
            dry_run: opts.dry_run,
            mirror_images: true,
            defer_mirror,
        },
        Some(cfg),
    )
}

/// Angebots-Abruf für genau eine Filiale — ohne Store-Finder, die ID steht
/// schon fest. In Produktion `stores::fetch_offers` über `Store::from_chain`;
/// in Tests ein Stub ohne Netz.
pub type BranchScraper<'a> = dyn Fn(&Branch) -> Result<Vec<Offer>> + 'a;

/// Genau eine Filiale syncen: Angebote holen, Filiale melden, pushen.
///
/// Der Unterschied zu [`sync_region`] ist nicht die Menge, sondern der
/// Schlüssel: Dort bestimmt die PLZ über den Store-Finder, welche Filiale
/// gemeint ist (`.first()`), hier hat der Nutzer sie im Verzeichnis selbst
/// ausgewählt. Zwei REWE-Filialen in derselben PLZ sind darüber
/// unterscheidbar — im PLZ-Weg waren sie es nie.
pub fn run_branch(
    opts: &SyncOptions,
    cfg: Option<&PushConfig>,
    scraper: &BranchScraper,
    market_id: &str,
) -> Result<()> {
    let cfg = match cfg {
        Some(c) => c,
        None => &push::config_from_env()?,
    };
    let branch = branches::fetch_branch(cfg, market_id)?;

    // Ohne PLZ kein Push: `offers.region` ist bis Phase 12 das Feld, über das
    // die App filtert, und der Lidl-Abruf braucht sie ohnehin. Im Verzeichnis
    // trägt sie jede Zeile — fehlt sie doch einmal, ist die Verzeichniszeile
    // kaputt und nicht der Lauf.
    let plz = branch.plz.clone().with_context(|| {
        format!(
            "Filiale {market_id} ({}) hat keine PLZ im Verzeichnis — \
             `smartshop branches-sync` für dieses Gebiet neu laufen lassen.",
            branch.name
        )
    })?;
    println!(
        "Filial-Sync: {} {} (ID {market_id}, PLZ {plz})",
        branch.chain, branch.name,
    );

    {
        let conn = db::open(&opts.db_path)?;
        conn.execute("DELETE FROM offers", [])
            .context("Lokale offers-Tabelle konnte nicht geleert werden")?;
    }

    let market = branch.as_market().with_chain(&branch.chain);
    let offers = scraper(&branch)
        .with_context(|| format!("Angebote von {} {} laden", branch.chain, branch.name))?;
    println!("{} {}: {} Angebote gefunden.", branch.chain, branch.name, offers.len());
    if offers.is_empty() {
        bail!("{} {}: Scraper lieferte 0 Angebote.", branch.chain, branch.name);
    }
    save_offers(&opts.db_path, &market, &offers)?;

    if opts.dry_run {
        println!("Dry-Run — Filiale wird nicht nach markets gemeldet.");
    } else {
        // `markets` führt bis Phase 12 die EINE Filiale je (Kette, PLZ) — die
        // App liest daraus, welche Ketten es in einer Region gibt. Wer diese
        // Filiale anfordert, will genau sie sehen, also wird sie dort
        // eingetragen. Das Verzeichnis in `branches` bleibt davon unberührt.
        upsert_markets(
            cfg,
            &[serde_json::json!({
                "chain": branch.chain,
                "branch_name": branch.name,
                "market_id": branch.market_id,
                "plz": plz,
                "lat": branch.lat,
                "lon": branch.lon,
                "updated_at": chrono::Utc::now().to_rfc3339(),
            })],
        )?;
    }

    push::run(
        &PushOptions {
            db_path: opts.db_path.clone(),
            chain: None,
            region: Some(plz),
            dry_run: opts.dry_run,
            mirror_images: true,
            // Wie beim On-Demand-Sync einer PLZ: Hier wartet gerade jemand in
            // der App, Angebote zuerst, Bilder danach.
            defer_mirror: true,
        },
        Some(cfg),
    )?;

    if !opts.dry_run {
        mark_branch_synced(cfg, market_id)?;
    }
    Ok(())
}

/// Die Anforderung als erledigt melden (`branch_requests.last_synced`).
///
/// Ohne diesen Schreibvorgang pollt die App bis zum Timeout: Sie sieht ihre
/// eigene Zeile mit `last_synced: null` und weiß nicht, dass die Angebote
/// längst da sind. Analog zum Region-Cache, den `push::run` am Ende setzt.
///
/// Der Upsert legt die Zeile auch an, wenn sie fehlt — ein Lauf von Hand hat
/// keine Anforderung vorher. Der Trigger aus migration_v14 lässt sich davon
/// nicht auslösen: Er steigt aus, sobald `last_synced` gesetzt ist.
fn mark_branch_synced(cfg: &PushConfig, market_id: &str) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let resp = auth(cfg, client.post(format!("{}/rest/v1/branch_requests", cfg.base_url)))
        .query(&[("on_conflict", "market_id")])
        .header("Prefer", "resolution=merge-duplicates")
        .json(&serde_json::json!([{
            "market_id": market_id,
            "last_synced": chrono::Utc::now().to_rfc3339(),
        }]))
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    check_response(&format!("Filiale {market_id} als gesynct eintragen"), resp)
}

/// Eine PLZ in `regions` registrieren, falls sie noch fehlt (409 = schon da).
fn register_region(cfg: &PushConfig, plz: &str) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let resp = auth(cfg, client.post(format!("{}/rest/v1/regions", cfg.base_url)))
        .json(&serde_json::json!({ "plz": plz }))
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    if resp.status().as_u16() == 409 {
        return Ok(());
    }
    check_response(&format!("Registrieren der Region {plz}"), resp)
}

/// Alle aktiven Regionen aus Supabase syncen. Fehler einzelner Regionen
/// brechen den Lauf nicht ab; Exit-Fehler nur, wenn ALLE Regionen scheitern.
/// Mit `only` wird genau eine PLZ gesynct (und vorher registriert): dort
/// zählt Sekunden bis zur ersten Anzeige — nationale Ketten werden vorab aus
/// vorhandenen Daten kopiert und Bilder erst nach dem Offers-Upsert gespiegelt.
pub fn run(
    opts: &SyncOptions,
    cfg: Option<&PushConfig>,
    fetcher: &Fetcher,
    finder: &BranchFinder,
) -> Result<()> {
    let cfg = match cfg {
        Some(c) => c,
        None => &push::config_from_env()?,
    };

    if let Some(plz) = &opts.only {
        if !opts.dry_run {
            register_region(cfg, plz)?;
            seed_national_chains(cfg, finder, plz);
        }
        println!("On-Demand-Sync: nur Region {plz}");
        return sync_region(opts, cfg, fetcher, plz, true)
            .with_context(|| format!("Region {plz} fehlgeschlagen"));
    }

    let regions = fetch_regions(cfg)?;
    if regions.is_empty() {
        bail!("Keine aktiven Regionen in Supabase gefunden.");
    }

    if regions.len() > opts.max_regions {
        let skipped: Vec<&str> = regions[opts.max_regions..]
            .iter()
            .map(|r| r.plz.as_str())
            .collect();
        println!(
            "{} Regionen angefordert, Limit {} — übersprungen: {}",
            regions.len(),
            opts.max_regions,
            skipped.join(", ")
        );
    }

    let selected = &regions[..regions.len().min(opts.max_regions)];
    println!(
        "Synce {} Region(en): {}",
        selected.len(),
        selected.iter().map(|r| r.plz.as_str()).collect::<Vec<_>>().join(", ")
    );

    let mut failures: Vec<(String, String)> = Vec::new();
    for region in selected {
        println!("\n=== Region {} ===", region.plz);
        if let Err(e) = sync_region(opts, cfg, fetcher, &region.plz, false) {
            eprintln!("Region {} fehlgeschlagen: {e:#}", region.plz);
            failures.push((region.plz.clone(), format!("{e:#}")));
        }
    }

    let ok = selected.len() - failures.len();
    println!("\nFertig: {ok}/{} Region(en) erfolgreich gesynct.", selected.len());
    if ok == 0 {
        bail!(
            "Alle {} Region(en) fehlgeschlagen: {}",
            selected.len(),
            failures
                .iter()
                .map(|(plz, e)| format!("{plz} ({e})"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(())
}

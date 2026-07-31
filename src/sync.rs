use anyhow::{Context, Result, bail};

use crate::branches;
use crate::db;
use crate::models::{Branch, Market, Offer};
use crate::push::{self, PushConfig, PushOptions};
use crate::stores::{Store, save_offers};

/// Speichert diese Kette ihren Katalog bundesweit unter einer einzigen
/// Filiale? Nimmt den Anzeigenamen aus `offers.market`, damit der Aufrufer
/// nicht selbst auf [`Store`] abbilden muss.
pub fn stores_nationally(chain: &str) -> bool {
    Store::from_chain(chain).is_some_and(Store::stores_nationally)
}

/// Alle Ketten mit bundesweitem Katalog (heute ALDI Nord und ALDI SÜD).
pub fn national_stores() -> Vec<Store> {
    Store::ALL.into_iter().filter(|s| s.stores_nationally()).collect()
}

/// Angebots-Abruf einer bundesweiten Kette: Store + National-Markt rein,
/// Angebote raus. In Produktion `stores::fetch_offers`; in Tests ein Stub.
pub type NationalScraper<'a> = dyn Fn(Store, &Market) -> Result<Vec<Offer>> + 'a;

pub struct SyncOptions {
    pub db_path: String,
    pub dry_run: bool,
    /// Höchstens so viele Filialen pro Lauf syncen; weitere werden geloggt
    /// und übersprungen.
    pub max_branches: usize,
    /// Nur diese eine Filiale syncen (`run_branch`).
    pub market_id: Option<String>,
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

/// Die gefundene Filiale nach `public.markets` melden — die App liest daraus
/// im Rückfall, welche Ketten es in einem Gebiet gibt, wenn das Verzeichnis
/// das Gebiet noch nicht kennt.
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
    national: &NationalScraper,
    market_id: &str,
) -> Result<()> {
    let cfg = match cfg {
        Some(c) => c,
        None => &push::config_from_env()?,
    };
    let branch = branches::fetch_branch(cfg, market_id)?;

    // Eine ALDI-Filiale kann man anfordern, aber nicht einzeln scrapen — die
    // Angebots-APIs beider ALDIs kennen keinen Filialbezug. Statt so zu tun
    // als ob, wird der bundesweite Katalog aufgefrischt und die Anforderung
    // damit beantwortet: Für die App ist das Ergebnis dasselbe, sie sieht
    // die Angebote unter jeder Filiale dieser Kette.
    if let Some(store) = Store::from_chain(&branch.chain).filter(|s| s.stores_nationally()) {
        println!(
            "Filial-Sync: {} {} (ID {market_id}) — Angebote liegen bundesweit, frische ich auf.",
            branch.chain, branch.name,
        );
        return sync_national(opts, cfg, national, &[store], Some(market_id));
    }

    // Die PLZ braucht nur noch der Lidl-Abruf (die Prospekt-Suche filtert
    // über sie, nicht über die Filial-ID). Im Verzeichnis trägt sie jede
    // Zeile — fehlt sie doch einmal, ist die Verzeichniszeile kaputt und
    // nicht der Lauf.
    let plz = branch.plz.clone().with_context(|| {
        format!(
            "Filiale {market_id} ({}) hat keine PLZ im Verzeichnis — \
             `lechariot branches-sync` für dieses Gebiet neu laufen lassen.",
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
            nationwide: false,
            branch_id: Some(branch.market_id.clone()),
            dry_run: opts.dry_run,
            // Seit 2026-07-27 werden Bilder nicht mehr gespiegelt: Die App
            // lädt die Händler-CDN-URLs direkt (Hotlinking frei zugänglicher
            // Inhalte statt eigener Kopien — urheberrechtlich der sichere
            // Weg, und der Storage-Egress entfällt). Spiegeln bleibt als
            // Opt-in über `push --mirror-images` erhalten.
            mirror_images: false,
            defer_mirror: false,
        },
        Some(cfg),
    )
}

/// Die bundesweiten Ketten genau einmal syncen — ohne Region, unter ihrer
/// National-Filiale (`ALDI_NORD_DE` / `ALDI_SUED_DE`).
///
/// Bis Phase 12 lief ALDI im Regions-Sync mit und bekam pro PLZ eine eigene
/// Kopie desselben Katalogs: 2.965 Zeilen für rund 320 Angebote. Schlimmer
/// als die Menge war die Aussage — jede Kopie behauptete, das Angebot gelte
/// für *diese* Filiale, obwohl der Scraper nie eine Filiale gefragt hat. Die
/// beiden ALDI-Angebots-APIs liefern schlicht keinen Filialbezug.
///
/// Möglich wurde das erst, als die App auf Filialen umgestellt war: Eine
/// Zeile ohne Region hätte der bisherige `region=in.(…)`-Filter nie geladen.
///
/// `stores` schränkt auf einzelne Ketten ein (der Filial-Sync fordert nur
/// eine an); `branch_id` meldet die auslösende Anforderung als erledigt.
/// Fehler einzelner Ketten warnen nur — eine kaputte ALDI-Seite darf die
/// Nightly nicht abbrechen.
pub fn sync_national(
    opts: &SyncOptions,
    cfg: &PushConfig,
    scraper: &NationalScraper,
    stores: &[Store],
    branch_id: Option<&str>,
) -> Result<()> {
    {
        let conn = db::open(&opts.db_path)?;
        conn.execute("DELETE FROM offers", [])
            .context("Lokale offers-Tabelle konnte nicht geleert werden")?;
    }

    let mut saved = 0usize;
    for store in stores {
        let Some(market) = store.national_market() else {
            bail!("{} speichert nicht bundesweit — sync_national ist hier falsch.", store.chain());
        };
        match scraper(*store, &market) {
            Ok(offers) if offers.is_empty() => {
                eprintln!("WARNUNG [{}]: Scraper lieferte 0 Angebote.", store.chain());
            }
            Ok(offers) => {
                println!("[{}] {} Angebote gefunden (bundesweit).", store.chain(), offers.len());
                match save_offers(&opts.db_path, &market, &offers) {
                    Ok(()) => saved += offers.len(),
                    Err(e) => eprintln!("[{}] Speichern fehlgeschlagen: {e:#}", store.chain()),
                }
            }
            Err(e) => eprintln!("WARNUNG [{}]: {e:#}", store.chain()),
        }
    }

    if saved == 0 {
        // Kein Abbruch: Die zuletzt gepushten Zeilen stehen weiter in
        // Supabase und sind noch gültig. Ein leerer Push würde daran nichts
        // verbessern, ein Fehler-Exit nur den Rest des Laufs verhindern.
        eprintln!("WARNUNG: keine bundesweiten Angebote geholt — nichts zu pushen.");
        return Ok(());
    }

    push::run(
        &PushOptions {
            db_path: opts.db_path.clone(),
            chain: None,
            // Der Punkt der ganzen Übung.
            nationwide: true,
            branch_id: branch_id.map(String::from),
            dry_run: opts.dry_run,
            // Kein Spiegeln mehr — siehe sync_branch: die App lädt die
            // Händler-URLs direkt.
            mirror_images: false,
            defer_mirror: false,
        },
        Some(cfg),
    )
}

/// Die Filialen, die jemand tatsächlich benutzt — aus zwei Quellen:
///
/// * `branch_requests` (aktiv): die ausdrückliche Warteschlange. Wer im Picker
///   eine Filiale wählt, die das Backend nie geholt hat, landet hier.
/// * `user_profiles.branch_ids`: die Filialen der Einwilligenden. Genau dafür
///   ist die Spalte da (Migration v15) — ohne sie ließe sich nicht sagen,
///   welche Läden überhaupt gebraucht werden.
///
/// Der Filial-Sync bediente bis hierher nur die Anforderung selbst und lief
/// über `workflow_dispatch`. Eine so geholte Filiale wurde also **nie wieder**
/// aufgefrischt; nach Ablauf ihrer Woche stand sie leer da, ohne dass
/// irgendwo etwas fehlgeschlagen wäre.
///
/// Fällt EINE der beiden Quellen aus (Tabelle fehlt, Recht fehlt), läuft es
/// mit der anderen weiter. Fallen **beide** aus, ist das kein „niemand hat
/// etwas gewählt", sondern „ich konnte nicht fragen" — und das muss ein
/// Fehler sein, sonst meldet ein Lauf gegen eine kaputte Datenbank Erfolg.
///
/// **Die Reihenfolge ist die Warteschlange**, siehe [`queue_order`]. Sie war
/// bis hierher alphabetisch (`BTreeSet`), und weil [`run`] die Liste vorn
/// abschneidet, entschied damit der Anfangsbuchstabe, wer gesynct wird. Am
/// 31.07.2026 fielen so jede Nacht dieselben sieben Filialen hinten herunter,
/// darunter alle drei Lidl-Märkte (`LIDL_*` sortiert hinter `DE*`) — genau
/// die, die am längsten nicht dran waren.
pub fn fetch_chosen_branches(cfg: &PushConfig) -> Result<Vec<String>> {
    let client = reqwest::blocking::Client::new();
    let mut queued: Vec<QueuedBranch> = Vec::new();
    let mut extra: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut answered = 0usize;

    let resp = auth(cfg, client.get(format!("{}/rest/v1/branch_requests", cfg.base_url)))
        .query(&[
            ("select", "market_id,last_synced,requested_at"),
            ("active", "eq.true"),
        ])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    if resp.status().is_success() {
        let rows: Vec<serde_json::Value> =
            resp.json().context("branch_requests: Antwort ist kein gültiges JSON")?;
        for row in &rows {
            let Some(market_id) = row.get("market_id").and_then(|v| v.as_str()) else { continue };
            queued.push(QueuedBranch {
                market_id: market_id.to_string(),
                last_synced: row.get("last_synced").and_then(|v| v.as_str()).map(String::from),
                requested_at: row.get("requested_at").and_then(|v| v.as_str()).map(String::from),
            });
        }
        answered += 1;
    } else {
        eprintln!("WARNUNG: branch_requests nicht lesbar (HTTP {})", resp.status());
    }

    let resp = auth(cfg, client.get(format!("{}/rest/v1/user_profiles", cfg.base_url)))
        .query(&[("select", "branch_ids")])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    if resp.status().is_success() {
        let rows: Vec<serde_json::Value> =
            resp.json().context("user_profiles: Antwort ist kein gültiges JSON")?;
        for row in &rows {
            let Some(list) = row.get("branch_ids").and_then(|v| v.as_array()) else { continue };
            extra.extend(list.iter().filter_map(|v| v.as_str().map(String::from)));
        }
        answered += 1;
    } else {
        eprintln!("WARNUNG: user_profiles nicht lesbar (HTTP {})", resp.status());
    }

    if answered == 0 {
        bail!(
            "Weder branch_requests noch user_profiles waren lesbar — \
             die Liste der gewählten Filialen ist damit unbekannt, nicht leer."
        );
    }

    // Filialen, die nur aus einem Profil kommen, haben noch keine
    // `branch_requests`-Zeile — der Push legt sie beim ersten erfolgreichen
    // Lauf an (`push.rs`, „Anforderung als erledigt melden"). Bis dahin
    // zählen sie als nie gesynct und stehen deshalb vorn.
    for id in extra {
        if !queued.iter().any(|q| q.market_id == id) {
            queued.push(QueuedBranch { market_id: id, last_synced: None, requested_at: None });
        }
    }

    Ok(queue_order(queued))
}

/// Eine gewählte Filiale mit dem, was ihren Platz in der Warteschlange
/// bestimmt.
#[derive(Debug, Clone)]
pub struct QueuedBranch {
    pub market_id: String,
    /// `None` = noch nie gesynct. Die App pollt genau diese Spalte und wartet,
    /// solange sie null ist (Migration v14).
    pub last_synced: Option<String>,
    pub requested_at: Option<String>,
}

/// Wer am längsten nicht dran war, kommt zuerst.
///
/// Die Reihenfolge steht seit Migration v14 im Schema — der Index heißt
/// `branch_requests_pending_idx` und lautet `(last_synced nulls first,
/// requested_at)`. Sie wird hier trotzdem im Programm hergestellt und nicht
/// über `order=` an PostgREST delegiert: Sortiert der Server, prüft ein Test
/// gegen einen Mock nur noch, dass die Antwort unverändert durchgereicht wird,
/// und bestünde auch ohne jede Sortierung.
///
/// **Nie gesynct steht vorn**, nicht hinten. Null heißt nicht „unwichtig",
/// sondern „die App wartet auf den ersten Lauf" — das ist der einzige Fall,
/// in dem ein Nutzer gerade zusieht.
///
/// Innerhalb dieser Gruppe kommt zuerst, wer ausdrücklich angefordert wurde
/// (ältere Anforderung zuerst), danach die Filialen, die nur in einem Profil
/// stehen: Hinter einer Anforderung wartet gerade jemand, hinter einem
/// Profileintrag niemand. Letzte Stufe ist die ID — nicht als Rangfolge,
/// sondern damit zwei Läufe über denselben Daten dieselbe Liste ergeben.
///
/// Doppelte IDs kann es nicht geben: `branch_requests.market_id` ist
/// Primärschlüssel, und die Profil-IDs werden nur übernommen, wenn noch keine
/// Anforderung sie trägt.
pub fn queue_order(mut branches: Vec<QueuedBranch>) -> Vec<String> {
    branches.sort_by(|a, b| {
        Due::of(&a.last_synced)
            .cmp(&Due::of(&b.last_synced))
            // Hier andersherum als bei `last_synced`: ohne Anforderungszeit
            // nach hinten (siehe oben).
            .then_with(|| a.requested_at.is_none().cmp(&b.requested_at.is_none()))
            .then_with(|| Due::of(&a.requested_at).cmp(&Due::of(&b.requested_at)))
            .then_with(|| a.market_id.cmp(&b.market_id))
    });
    branches.into_iter().map(|b| b.market_id).collect()
}

/// Wie dringend eine Filiale drankommen muss. Die Reihenfolge der Varianten
/// **ist** die Sortierung (`derive(Ord)` vergleicht erst die Variante).
///
/// `Unlesbar` steht ausdrücklich zwischen `Nie` und `Am`, statt mit `Nie`
/// zusammenzufallen: Ein Zeitstempel, den niemand parsen kann, wird auch nach
/// dem nächsten Lauf nicht lesbar sein. Läge er ganz vorn, hätte diese Filiale
/// **dauerhaft** Vorrang und schnitte die Übrigen ab — derselbe stille
/// Datenverlust, den dieser Commit behebt, nur andersherum. Also: bald dran,
/// aber hinter denen, auf die jemand wartet, und laut gemeldet.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Due {
    Nie,
    Unlesbar,
    Am(chrono::DateTime<chrono::FixedOffset>),
}

impl Due {
    fn of(raw: &Option<String>) -> Due {
        let Some(raw) = raw.as_deref() else { return Due::Nie };
        match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(t) => Due::Am(t),
            Err(e) => {
                eprintln!("WARNUNG: Zeitstempel '{raw}' nicht lesbar ({e}) — Filiale wird vorgezogen.");
                Due::Unlesbar
            }
        }
    }
}

/// Der komplette nächtliche Lauf: bundesweite Ketten einmal, danach jede
/// Filiale, die jemand benutzt.
///
/// Der Regionsweg ist mit Migration v16 weg. Er stammte aus der Zeit, als ein
/// Angebot einer PLZ gehörte: Die App trug eine PLZ ein, ein Trigger stieß den
/// Sync an, und der scrapte je Kette die dem PLZ-Zentrum nächste Filiale.
/// Seit v13 gehören Angebote der Filiale, und seit Phase 12 wählt der Nutzer
/// sie selbst — die PLZ sagte zuletzt nur noch, über welche Suche der Scraper
/// die Filiale gefunden hat.
///
/// Fehler einzelner Filialen brechen den Lauf nicht ab; Exit-Fehler nur, wenn
/// ALLE scheitern.
pub fn run(
    opts: &SyncOptions,
    cfg: Option<&PushConfig>,
    branch_scraper: &BranchScraper,
    national: &NationalScraper,
) -> Result<()> {
    let cfg = match cfg {
        Some(c) => c,
        None => &push::config_from_env()?,
    };

    // Zuerst die bundesweiten Ketten — sie hängen an keiner Filiale und gelten
    // danach überall. Fehler warnen hier nur (siehe `sync_national`), der
    // Filial-Durchgang läuft in jedem Fall weiter.
    println!("=== Bundesweite Ketten ===");
    if let Err(e) = sync_national(opts, cfg, national, &national_stores(), None) {
        eprintln!("WARNUNG: Bundesweiter Sync fehlgeschlagen: {e:#}");
    }

    let branches = fetch_chosen_branches(cfg)?;
    if branches.is_empty() {
        println!("Keine gewählten Filialen — nur die bundesweiten Ketten gesynct.");
        return Ok(());
    }

    // Eine übersprungene Filiale ist stiller Datenverlust — dieselbe Klasse,
    // die `branches.yml` schon als Annotation nach oben holt. Deshalb WARNUNG
    // auf stderr und nicht `println!`: Am 31.07.2026 hat ein grüner Lauf drei
    // Lidl-Märkte fallen lassen, und die Zeile stand nur im Log.
    let selected: &[String] = if branches.len() > opts.max_branches {
        eprintln!(
            "WARNUNG: {} Filialen angefordert, Limit {} — übersprungen: {}",
            branches.len(),
            opts.max_branches,
            branches[opts.max_branches..].join(", ")
        );
        &branches[..opts.max_branches]
    } else {
        &branches
    };

    println!("\nSynce {} Filiale(n): {}", selected.len(), selected.join(", "));

    let mut failures: Vec<(String, String)> = Vec::new();
    for market_id in selected {
        println!("\n=== Filiale {market_id} ===");
        if let Err(e) = run_branch(opts, Some(cfg), branch_scraper, national, market_id) {
            eprintln!("Filiale {market_id} fehlgeschlagen: {e:#}");
            failures.push((market_id.clone(), format!("{e:#}")));
        }
    }

    let ok = selected.len() - failures.len();
    println!("\nFertig: {ok}/{} Filiale(n) erfolgreich gesynct.", selected.len());
    if ok == 0 {
        bail!(
            "Alle {} Filiale(n) fehlgeschlagen: {}",
            selected.len(),
            failures
                .iter()
                .map(|(id, e)| format!("{id} ({e})"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(())
}

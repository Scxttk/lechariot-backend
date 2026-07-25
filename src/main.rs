use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

use smartshop::models::Offer;
use smartshop::stores::{Store, save_offers, scrape_store};
use smartshop::{db, units};

#[derive(Parser)]
#[command(name = "smartshop", about = "Supermarkt-Angebote scrapen und speichern")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Json,
    Csv,
}

#[derive(Subcommand)]
enum Command {
    /// Angebote eines Markts abrufen und speichern
    Fetch {
        /// Postleitzahl des Markts
        #[arg(long)]
        zip: String,

        /// Supermarkt
        #[arg(long, value_enum, default_value_t = Store::Rewe, conflicts_with = "all_stores")]
        store: Store,

        /// Alle Supermärkte nacheinander abrufen
        #[arg(long, default_value_t = false)]
        all_stores: bool,

        /// Pfad zum Rewe TLS-Zertifikat (PEM)
        #[arg(long, default_value = "cert.pem")]
        cert: String,

        /// Pfad zum privaten Schlüssel
        #[arg(long, default_value = "private.key")]
        key: String,

        /// Nur ausgeben, nicht speichern
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Nach dem Abruf die Watchlist gegen die neuen Angebote prüfen
        #[arg(long, default_value_t = false, conflicts_with = "dry_run")]
        notify: bool,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Gespeicherte Angebote nach Titel durchsuchen
    Search {
        /// Suchbegriff (Teilstring des Titels)
        query: String,

        /// Nur Angebote bis zu diesem Preis
        #[arg(long)]
        max_price: Option<f64>,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Preise eines Produkts über alle gespeicherten Märkte vergleichen
    Compare {
        /// Suchbegriff (Teilstring des Titels)
        query: String,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Gespeicherte Angebote als JSON oder CSV exportieren
    Export {
        /// Ausgabeformat
        #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
        format: ExportFormat,

        /// Nur Angebote, deren Titel/Untertitel den Suchbegriff enthält
        #[arg(long)]
        query: Option<String>,

        /// In Datei schreiben statt auf stdout
        #[arg(long)]
        out: Option<String>,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Statistiken über die gespeicherten Angebote anzeigen
    Stats {
        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Watchlist verwalten: Produkte beobachten und Treffer finden
    Watch {
        #[command(subcommand)]
        action: WatchAction,
    },
    /// Einkaufsliste verwalten und günstigste Angebote finden
    List {
        #[command(subcommand)]
        action: ListAction,
    },
    /// Preissenkungen anzeigen: Produkte, die günstiger geworden sind
    Deals {
        /// Nur Senkungen der letzten N Tage
        #[arg(long)]
        since: Option<i64>,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Lokales Web-Dashboard und HTTP-API für die gespeicherten Angebote starten
    Serve {
        /// Port für den HTTP-Server
        #[arg(long, default_value_t = 8080)]
        port: u16,

        /// Adresse, an die gebunden wird. Standard: nur lokal (Loopback).
        /// Für Zugriff aus dem LAN bewusst z. B. `--host 0.0.0.0` setzen —
        /// dann ist das Dashboard ohne Authentifizierung für alle im Netz
        /// erreichbar.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Gespeicherte Angebote nach Supabase hochladen (public.offers)
    Push {
        /// Nur diesen Supermarkt pushen (Standard: alle)
        #[arg(long, value_enum, conflicts_with = "all_stores")]
        store: Option<Store>,

        /// Alle Supermärkte pushen (Standardverhalten, explizit)
        #[arg(long, default_value_t = false)]
        all_stores: bool,

        /// PLZ, aus der die Angebote stammen (Pflicht außer bei --dry-run
        /// oder --national)
        #[arg(long)]
        region: Option<String>,

        /// Bundesweit speichern statt für eine PLZ: die Zeilen bekommen
        /// `region = NULL` und gelten für jede Filiale ihrer Kette. Nur für
        /// Ketten mit bundesweit identischem Katalog (ALDI Nord, ALDI SÜD) —
        /// bei allen anderen wären die Angebote schlicht falsch.
        #[arg(long, default_value_t = false, conflicts_with = "region")]
        national: bool,

        /// Nur zeigen, was hochgeladen würde — kein Netzwerkzugriff
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Produktbilder NICHT in den Storage-Bucket spiegeln (Händler-URLs behalten)
        #[arg(long, default_value_t = false)]
        no_mirror_images: bool,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Alle aktiven Regionen aus Supabase abrufen und syncen (fetch + push)
    SyncRegions {
        /// Höchstens so viele Regionen pro Lauf syncen
        #[arg(long, default_value_t = 10)]
        max_regions: usize,

        /// Nur diese eine PLZ syncen (wird bei Bedarf registriert)
        #[arg(long, value_name = "PLZ")]
        only: Option<String>,

        /// Nur diese eine Filiale syncen (ID aus `public.branches`).
        /// Angebote gehören der Filiale, nicht der Kette in einer PLZ —
        /// dieser Weg trifft auch die zweite REWE derselben PLZ.
        #[arg(long, value_name = "ID", conflicts_with = "only")]
        market_id: Option<String>,

        /// Nur die bundesweiten Ketten (ALDI Nord, ALDI SÜD) syncen und
        /// danach aufhören. Deren Katalog hängt an keiner Region — dieser
        /// Lauf braucht weder Regionsliste noch Store-Finder und ist in
        /// Sekunden durch.
        #[arg(long, default_value_t = false, conflicts_with_all = ["only", "market_id"])]
        national_only: bool,

        /// Pfad zum Rewe TLS-Zertifikat (PEM)
        #[arg(long, default_value = "cert.pem")]
        cert: String,

        /// Pfad zum privaten Schlüssel
        #[arg(long, default_value = "private.key")]
        key: String,

        /// Nur zeigen, was hochgeladen würde — keine Supabase-Writes
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Filialverzeichnis (`public.branches`) befüllen.
    ///
    /// Ohne `--area` nur die beiden Ketten, deren komplettes Verzeichnis einen
    /// einzigen Request kostet (Kaufland, Penny). Gebiete kosten pro Kette
    /// eine Suche und werden deshalb angefordert, nicht geraten.
    /// Braucht SUPABASE_URL und SUPABASE_SERVICE_KEY in der Umgebung.
    BranchesSync {
        /// PLZ, deren Umkreis geholt wird (mehrfach angebbar)
        #[arg(long, value_name = "PLZ")]
        area: Vec<String>,

        /// Alle aktiven Regionen aus Supabase als Gebiete verwenden
        #[arg(long, default_value_t = false)]
        from_regions: bool,

        /// Kaufland und Penny nicht mitziehen
        #[arg(long, default_value_t = false)]
        skip_national: bool,

        /// Umkreis der Gebietssuche in km
        #[arg(long, default_value_t = smartshop::branches::AREA_RADIUS_KM)]
        radius_km: f64,

        /// Pfad zum Rewe TLS-Zertifikat (PEM)
        #[arg(long, default_value = "cert.pem")]
        cert: String,

        /// Pfad zum privaten Schlüssel
        #[arg(long, default_value = "private.key")]
        key: String,

        /// Nur zeigen, was geschrieben würde — keine Supabase-Writes
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Preisverlauf eines Produkts anzeigen
    History {
        /// Suchbegriff (Teilstring des Titels)
        query: String,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Verwaiste Bilder aus dem Storage-Bucket `offer-images` entfernen.
    /// Standard: Dry-Run (zeigt nur, was gelöscht würde). Braucht
    /// SUPABASE_URL und SUPABASE_SERVICE_KEY in der Umgebung.
    PruneImages {
        /// Tatsächlich löschen. Ohne dieses Flag nur anzeigen (Dry-Run).
        #[arg(long, default_value_t = false)]
        execute: bool,

        /// Objekte, die jünger als so viele Tage sind, nie löschen — schützt
        /// gerade gespiegelte Bilder eines laufenden Push/Sync.
        #[arg(long, default_value_t = 7)]
        min_age_days: i64,
    },
    /// Alte Wochen aus `public.price_history` löschen.
    ///
    /// Die Historie rotiert nicht von selbst: Jeder Push legt eine weitere
    /// Wochen-Zeile je Produkt und Filiale an. Mit dem Filial-Schlüssel
    /// multipliziert sich das mit jeder angeforderten Filiale.
    PruneHistory {
        /// Tatsächlich löschen. Ohne dieses Flag nur anzeigen (Dry-Run).
        #[arg(long, default_value_t = false)]
        execute: bool,

        /// So viele Wochen aufbewahren.
        #[arg(long, default_value_t = smartshop::prune::HISTORY_WEEKS)]
        weeks: i64,
    },
}

#[derive(Subcommand)]
enum WatchAction {
    /// Produkt zur Watchlist hinzufügen
    Add {
        /// Suchbegriff (Teilstring von Titel/Untertitel)
        query: String,

        /// Nur Treffer bis zu diesem Preis melden
        #[arg(long)]
        max_price: Option<f64>,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Alle Watchlist-Einträge anzeigen
    List {
        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Watchlist gegen die gespeicherten Angebote prüfen (Exit-Code 1 bei Treffern)
    Check {
        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Eintrag aus der Watchlist entfernen
    Remove {
        /// ID des Eintrags (siehe `watch list`)
        id: i64,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
}

#[derive(Subcommand)]
enum ListAction {
    /// Artikel auf die Einkaufsliste setzen
    Add {
        /// Artikelname
        item: String,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Artikel von der Einkaufsliste entfernen
    Remove {
        /// Artikelname
        item: String,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Einkaufsliste anzeigen
    Show {
        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Einkaufsliste leeren
    Clear {
        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Für jeden Artikel zeigen, wo er diese Woche am günstigsten ist
    Suggest {
        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Fetch { zip, store, all_stores, cert, key, dry_run, notify, db } => {
            if all_stores {
                fetch_all(zip, cert, key, dry_run, &db)?;
            } else {
                fetch(zip, store, cert, key, dry_run, &db)?;
            }
            if notify {
                notify_watchlist(&db)?;
            }
            Ok(())
        }
        Command::Search { query, max_price, db } => search(query, max_price, db),
        Command::Compare { query, db } => compare(query, db),
        Command::Export { format, query, out, db } => export(format, query, out, db),
        Command::Stats { db } => stats(db),
        Command::Watch { action } => watch(action),
        Command::List { action } => shopping_list(action),
        Command::Deals { since, db } => deals(since, db),
        Command::Serve { port, host, db } => smartshop::api::serve(&host, port, db),
        Command::Push { store, all_stores: _, region, national, dry_run, no_mirror_images, db } => {
            // Ohne PLZ und ohne --national landeten die Zeilen mit
            // `region = NULL` in der Datenbank und wären für jede Filiale
            // sichtbar — bei REWE oder Kaufland schlicht falsch. Der Schalter
            // erzwingt, dass „bundesweit" eine Entscheidung ist, kein
            // vergessenes Argument.
            if region.is_none() && !national && !dry_run {
                anyhow::bail!(
                    "--region <PLZ> ist Pflicht (außer bei --dry-run). \
                     Bundesweite Ketten wie ALDI brauchen stattdessen --national."
                );
            }
            let opts = smartshop::push::PushOptions {
                db_path: db,
                chain: store.map(|s| s.chain().to_string()),
                // Der Push von Hand bedient keine Filial-Anforderung.
                branch_id: None,
                region,
                dry_run,
                mirror_images: !no_mirror_images,
                defer_mirror: false,
            };
            smartshop::push::run(&opts, None)
        }
        Command::SyncRegions {
            max_regions,
            only,
            market_id,
            national_only,
            cert,
            key,
            dry_run,
            db,
        } => {
            let opts = smartshop::sync::SyncOptions {
                db_path: db,
                dry_run,
                max_regions,
                only,
                market_id: market_id.clone(),
            };
            // Bundesweite Ketten: Der National-Markt steht fest, es gibt
            // nichts zu suchen — nur den Katalog zu holen.
            let national = |store: Store, market: &smartshop::models::Market| {
                smartshop::stores::fetch_offers(store, market, "", &cert, &key)
            };
            if national_only {
                let cfg = smartshop::push::config_from_env()?;
                return smartshop::sync::sync_national(
                    &opts,
                    &cfg,
                    &national,
                    &smartshop::sync::national_stores(),
                    None,
                );
            }
            // Filial-Sync: Die Filiale steht fest, es gibt nichts zu suchen —
            // nur die Kette muss auf ihren Scraper zeigen. Der Voll-Lauf
            // benutzt dieselbe Closure für die selbst gewählten Filialen.
            let scraper = |branch: &smartshop::models::Branch| {
                let store = Store::from_chain(&branch.chain).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Filiale {} trägt die unbekannte Kette '{}'",
                        branch.market_id,
                        branch.chain
                    )
                })?;
                let zip = branch.plz.as_deref().unwrap_or_default();
                smartshop::stores::fetch_offers(
                    store,
                    &branch.as_market().with_chain(&branch.chain),
                    zip,
                    &cert,
                    &key,
                )
            };
            if let Some(market_id) = &market_id {
                return smartshop::sync::run_branch(&opts, None, &scraper, &national, market_id);
            }
            let fetcher = |plz: &str| {
                Store::ALL
                    .iter()
                    .map(|store| {
                        // Bundesweite Ketten werden hier nur noch gesucht:
                        // Ob ALDI in der Region vertreten ist, entscheidet
                        // der Store-Finder, die Angebote holt der
                        // bundesweite Lauf. Die leere Angebotsliste ist
                        // Absicht — `sync_region` erkennt die Kette daran
                        // nicht, sondern an `stores_nationally`.
                        let result = if store.stores_nationally() {
                            smartshop::stores::find_market(*store, plz, &cert, &key)
                                .map(|m| m.map(|m| (m, Vec::new())))
                        } else {
                            scrape_store(*store, plz, &cert, &key)
                        };
                        (store.chain().to_string(), result)
                    })
                    .collect()
            };
            smartshop::sync::run(&opts, None, &fetcher, &scraper, &national)
        }
        Command::BranchesSync {
            area,
            from_regions,
            skip_national,
            radius_km,
            cert,
            key,
            dry_run,
        } => {
            let cfg = smartshop::push::config_from_env()?;
            let mut areas = area;
            if from_regions {
                let from_db = smartshop::branches::areas_from_regions(&cfg)?;
                println!("{} aktive Region(en) aus Supabase als Gebiete.", from_db.len());
                areas.extend(from_db);
            }
            areas.sort();
            areas.dedup();
            let opts = smartshop::branches::DirectoryOptions {
                areas,
                national: !skip_national,
                radius_km,
                cert,
                key,
                dry_run,
            };
            smartshop::branches::sync(&cfg, &opts).map(|_| ())
        }
        Command::History { query, db } => history(query, db),
        Command::PruneImages { execute, min_age_days } => {
            let opts = smartshop::prune::PruneOptions { execute, min_age_days };
            smartshop::prune::run(&opts, None)
        }
        Command::PruneHistory { execute, weeks } => {
            let opts = smartshop::prune::HistoryPruneOptions { execute, weeks };
            smartshop::prune::run_history(&opts, None)
        }
    }
}

/// Alle Watches gegen die gespeicherten Angebote prüfen und Treffer gruppiert
/// ausgeben. Liefert true, wenn es mindestens einen Treffer gab.
fn print_watch_hits(conn: &rusqlite::Connection) -> Result<bool> {
    let watches = db::watches(conn)?;
    if watches.is_empty() {
        println!("Watchlist ist leer. Mit `smartshop watch add <Suchbegriff>` anlegen.");
        return Ok(false);
    }
    let mut any = false;
    for w in &watches {
        let hits = db::watch_hits(conn, w)?;
        let limit = w
            .max_price
            .map(|p| format!(" (bis {p:.2} €)"))
            .unwrap_or_default();
        if hits.is_empty() {
            println!("#{} '{}'{limit}: keine Treffer.", w.id, w.query);
            continue;
        }
        any = true;
        println!("#{} '{}'{limit}: {} Treffer", w.id, w.query, hits.len());
        for offer in &hits {
            println!("  {}", format_offer(offer));
        }
    }
    Ok(any)
}

fn shopping_list(action: ListAction) -> Result<()> {
    match action {
        ListAction::Add { item, db } => {
            let conn = db::open(&db)?;
            if db::list_add(&conn, &item)? {
                println!("'{item}' auf die Einkaufsliste gesetzt.");
            } else {
                println!("'{item}' steht schon auf der Einkaufsliste.");
            }
            Ok(())
        }
        ListAction::Remove { item, db } => {
            let conn = db::open(&db)?;
            if db::list_remove(&conn, &item)? {
                println!("'{item}' von der Einkaufsliste entfernt.");
            } else {
                println!("'{item}' steht nicht auf der Einkaufsliste.");
            }
            Ok(())
        }
        ListAction::Show { db } => {
            let conn = db::open(&db)?;
            let items = db::list_items(&conn)?;
            if items.is_empty() {
                println!("Einkaufsliste ist leer. Mit `smartshop list add <Artikel>` befüllen.");
                return Ok(());
            }
            println!("Einkaufsliste ({} Artikel):", items.len());
            for it in &items {
                println!("  - {}", it.item);
            }
            Ok(())
        }
        ListAction::Clear { db } => {
            let conn = db::open(&db)?;
            let n = db::list_clear(&conn)?;
            println!("Einkaufsliste geleert ({n} Artikel entfernt).");
            Ok(())
        }
        ListAction::Suggest { db } => suggest(&db),
    }
}

/// Für jeden Listen-Artikel das günstigste passende Angebot über alle Märkte
/// finden (Titel/Untertitel-Suche wie bei compare, Grundpreis aus units).
fn suggest(db: &str) -> Result<()> {
    let conn = db::open(db)?;
    let items = db::list_items(&conn)?;
    if items.is_empty() {
        println!("Einkaufsliste ist leer. Mit `smartshop list add <Artikel>` befüllen.");
        return Ok(());
    }
    let market_names: std::collections::HashMap<String, String> = db::markets(&conn)?
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();

    for it in &items {
        let mut offers = db::search_offers_broad(&conn, &it.item)?;
        offers.retain(|o| o.price.is_some());
        let Some(best) = offers.first() else {
            println!("{:<24} keine Angebote diese Woche", it.item);
            continue;
        };
        let market = market_names
            .get(&best.market_id)
            .map(String::as_str)
            .unwrap_or(best.market_id.as_str());
        let unit = units::derive_unit_price(
            best.price,
            &[best.subtitle.as_deref(), best.overline.as_deref(), Some(&best.title)],
        )
        .map(|up| format!("  ({})", up.format()))
        .unwrap_or_default();
        println!(
            "{:<24} {:.2} € bei {market} — {}{unit}",
            it.item,
            best.price.unwrap(),
            display_name(best),
        );
        // Ersparnis gegenüber dem teuersten anderen Markt zeigen —
        // nur für dasselbe Produkt (gleicher normalisierter Name)
        let best_key = units::normalize_name(&display_name(best));
        if let Some(worst) = offers
            .iter()
            .filter(|o| {
                o.market_id != best.market_id
                    && units::normalize_name(&display_name(o)) == best_key
            })
            .last()
        {
            if let (Some(bp), Some(wp)) = (best.price, worst.price) {
                if wp > bp {
                    let worst_market = market_names
                        .get(&worst.market_id)
                        .map(String::as_str)
                        .unwrap_or(worst.market_id.as_str());
                    println!("{:<24} spart {:.2} € gegenüber {worst_market}", "", wp - bp);
                }
            }
        }
    }
    Ok(())
}

fn watch(action: WatchAction) -> Result<()> {
    match action {
        WatchAction::Add { query, max_price, db } => {
            let conn = db::open(&db)?;
            let id = db::add_watch(&conn, &query, max_price)?;
            let limit = max_price
                .map(|p| format!(" (bis {p:.2} €)"))
                .unwrap_or_default();
            println!("Watch #{id} angelegt: '{query}'{limit}");
            Ok(())
        }
        WatchAction::List { db } => {
            let conn = db::open(&db)?;
            let watches = db::watches(&conn)?;
            if watches.is_empty() {
                println!("Watchlist ist leer. Mit `smartshop watch add <Suchbegriff>` anlegen.");
                return Ok(());
            }
            println!("  {:>4}  {:<30} {:>10}  {}", "ID", "Suchbegriff", "Max-Preis", "Angelegt");
            for w in &watches {
                let max = w
                    .max_price
                    .map(|p| format!("{p:.2} €"))
                    .unwrap_or_else(|| "-".to_string());
                println!("  {:>4}  {:<30} {:>10}  {}", w.id, w.query, max, w.created_at);
            }
            Ok(())
        }
        WatchAction::Check { db } => {
            let conn = db::open(&db)?;
            let hits = print_watch_hits(&conn)?;
            if hits {
                // Cron-tauglich: Exit-Code 1 signalisiert "es gibt Treffer"
                std::process::exit(1);
            }
            Ok(())
        }
        WatchAction::Remove { id, db } => {
            let conn = db::open(&db)?;
            if db::remove_watch(&conn, id)? {
                println!("Watch #{id} entfernt.");
            } else {
                println!("Kein Watch mit ID {id} gefunden.");
            }
            Ok(())
        }
    }
}

fn deals(since: Option<i64>, db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let drops = db::price_drops(&conn, since)?;
    if drops.is_empty() {
        let window = since.map(|d| format!(" in den letzten {d} Tagen")).unwrap_or_default();
        println!("Keine Preissenkungen{window} gefunden.");
        return Ok(());
    }
    let market_names: std::collections::HashMap<String, String> = db::markets(&conn)?
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();
    println!("Preissenkungen ({}):", drops.len());
    for d in &drops {
        let market = market_names
            .get(&d.market_id)
            .map(String::as_str)
            .unwrap_or(d.market_id.as_str());
        let pct = (1.0 - d.new_price / d.old_price) * 100.0;
        println!(
            "  -{:.2} € (-{pct:.0} %)  {} — {:.2} € statt {:.2} € ({market}, {} -> {})",
            d.old_price - d.new_price,
            d.title,
            d.new_price,
            d.old_price,
            d.old_seen_at,
            d.new_seen_at,
        );
    }
    Ok(())
}

fn history(query: String, db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let points = db::price_history(&conn, &query)?;
    if points.is_empty() {
        println!("Kein Preisverlauf für '{query}' gefunden.");
        return Ok(());
    }
    let mut current_title = String::new();
    for p in &points {
        if p.title != current_title {
            println!("{} ({})", p.title, p.market_id);
            current_title = p.title.clone();
        }
        let price = p.price.map(|v| format!("{v:.2} €")).unwrap_or_else(|| "-".to_string());
        println!("  {}  {}", p.seen_at, price);
    }
    Ok(())
}

fn notify_watchlist(db: &str) -> Result<()> {
    println!("\nDeals für deine Watchlist:");
    let conn = db::open(db)?;
    if !print_watch_hits(&conn)? {
        println!("Keine neuen Deals.");
    }
    Ok(())
}

fn fetch_all(zip: String, cert: String, key: String, dry_run: bool, db: &str) -> Result<()> {
    struct Row {
        store: &'static str,
        market: String,
        result: std::result::Result<usize, String>,
    }
    let mut rows = Vec::new();

    for store in Store::ALL {
        match scrape_store(store, &zip, &cert, &key) {
            Ok(None) => {
                println!("Keine {}-Filiale in der Nähe von {zip}.", store.label());
                rows.push(Row {
                    store: store.label(),
                    market: "-".to_string(),
                    result: Ok(0),
                });
            }
            Ok(Some((market, offers))) => {
                let count = offers.len();
                println!("{} Angebote gefunden.", count);
                if dry_run {
                    for offer in &offers {
                        println!("  {}", format_offer(offer));
                    }
                    rows.push(Row { store: store.label(), market: market.name, result: Ok(count) });
                } else {
                    match save_offers(db, &market, &offers) {
                        Ok(()) => {
                            println!("{count} Angebote in '{db}' gespeichert.");
                            rows.push(Row { store: store.label(), market: market.name, result: Ok(count) });
                        }
                        Err(e) => {
                            eprintln!("{}: Speichern fehlgeschlagen: {e:#}", store.label());
                            rows.push(Row { store: store.label(), market: market.name, result: Err(format!("{e:#}")) });
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("{}: Fehler: {e:#}", store.label());
                rows.push(Row { store: store.label(), market: "-".to_string(), result: Err(format!("{e:#}")) });
            }
        }
        println!();
    }

    println!("Zusammenfassung:");
    println!("  {:<10} {:<28} {}", "Markt", "Filiale", "Ergebnis");
    for row in &rows {
        let result = match &row.result {
            Ok(n) => format!("{n} Angebote"),
            Err(e) => {
                let first = e.lines().next().unwrap_or("Fehler");
                format!("FEHLER: {first}")
            }
        };
        println!("  {:<10} {:<28} {}", row.store, row.market, result);
    }
    Ok(())
}

fn fetch(zip: String, store: Store, cert: String, key: String, dry_run: bool, db: &str) -> Result<()> {
    let Some((market, offers)) = scrape_store(store, &zip, &cert, &key)? else {
        println!("Keine {}-Filiale in der Nähe von {zip}.", store.label());
        return Ok(());
    };
    println!("{} Angebote gefunden.", offers.len());

    if dry_run {
        for offer in &offers {
            println!("  {}", format_offer(offer));
        }
    } else {
        save_offers(db, &market, &offers)?;
        println!("{} Angebote in '{}' gespeichert.", offers.len(), db);
    }

    Ok(())
}

fn search(query: String, max_price: Option<f64>, db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let offers = db::search_offers(&conn, &query, max_price)?;
    if offers.is_empty() {
        println!("Keine Angebote für '{query}' gefunden.");
    } else {
        for offer in &offers {
            println!("  {}", format_offer(offer));
        }
        println!("{} Treffer.", offers.len());
    }
    Ok(())
}

// Anzeigename: Titel, plus Untertitel wenn er kein reiner Mengen-Text ist
// (Kaufland: Marke im Titel, Produktname im Untertitel).
fn display_name(offer: &Offer) -> String {
    match &offer.subtitle {
        Some(sub) if units::parse_quantity(sub).is_none() && !sub.is_empty() => {
            format!("{} {}", offer.title, sub)
        }
        _ => offer.title.clone(),
    }
}

fn compare(query: String, db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let offers = db::search_offers_broad(&conn, &query)?;
    if offers.is_empty() {
        println!("Keine Angebote für '{query}' gefunden.");
        return Ok(());
    }
    let market_names: std::collections::HashMap<String, String> = db::markets(&conn)?
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();

    // Nach normalisiertem Produktnamen gruppieren, Reihenfolge des Auftretens
    let mut groups: Vec<(String, Vec<&Offer>)> = Vec::new();
    for offer in &offers {
        let key = units::normalize_name(&display_name(offer));
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(offer),
            None => groups.push((key, vec![offer])),
        }
    }

    for (_, mut group) in groups {
        // Günstigster Markt zuerst; Angebote ohne Preis ans Ende
        group.sort_by(|a, b| {
            a.price
                .unwrap_or(f64::INFINITY)
                .total_cmp(&b.price.unwrap_or(f64::INFINITY))
        });
        println!("{}", display_name(group[0]));
        let mut printed = std::collections::HashSet::new();
        for offer in group {
            // Gleicher Markt + gleicher Preis (z. B. aktuelle und nächste Woche) nur einmal
            if !printed.insert((offer.market_id.clone(), offer.price.map(|p| (p * 100.0) as i64))) {
                continue;
            }
            let market = market_names
                .get(&offer.market_id)
                .map(String::as_str)
                .unwrap_or(offer.market_id.as_str());
            let price = offer
                .price
                .map(|p| format!("{p:.2} €"))
                .unwrap_or_else(|| "-".to_string());
            let unit = units::derive_unit_price(
                offer.price,
                &[offer.subtitle.as_deref(), offer.overline.as_deref(), Some(&offer.title)],
            )
            .map(|up| format!("  ({})", up.format()))
            .unwrap_or_default();
            let sub = offer.subtitle.as_ref().map(|s| format!(" ({s})")).unwrap_or_default();
            println!("  {market:<20} {price:>8}{unit}{sub}");
        }
    }
    Ok(())
}

fn stats(db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let stats = db::market_stats(&conn)?;
    if stats.is_empty() {
        println!("Keine Angebote gespeichert.");
        return Ok(());
    }

    println!("Angebote pro Markt:");
    println!("  {:<28} {:>8}  {:<23} {}", "Filiale", "Angebote", "Gültigkeit", "Ø Rabatt");
    for s in &stats {
        let range = match (&s.valid_from_min, &s.valid_until_max) {
            (None, None) => "-".to_string(),
            (f, u) => format!("{} – {}", f.as_deref().unwrap_or("?"), u.as_deref().unwrap_or("?")),
        };
        let discount = s
            .avg_discount_pct
            .map(|p| format!("{p:.0} %"))
            .unwrap_or_else(|| "-".to_string());
        println!("  {:<28} {:>8}  {:<23} {}", s.market_name, s.offer_count, range, discount);
    }

    let top = db::top_discounts(&conn, 10)?;
    if !top.is_empty() {
        println!("\nTop 10 Rabatte:");
        for (i, d) in top.iter().enumerate() {
            let sub = d.subtitle.as_ref().map(|s| format!(" {s}")).unwrap_or_default();
            println!(
                "  {:>2}. -{:.0} %  {}{} — {:.2} € statt {:.2} € ({})",
                i + 1,
                d.discount_pct,
                d.title,
                sub,
                d.price,
                d.regular_price,
                d.market_name
            );
        }
    }
    Ok(())
}

fn export(format: ExportFormat, query: Option<String>, out: Option<String>, db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let offers = db::export_offers(&conn, query.as_deref())?;

    let content = match format {
        ExportFormat::Json => serde_json::to_string_pretty(&offers)?,
        ExportFormat::Csv => offers_to_csv(&offers),
    };

    match out {
        Some(path) => {
            std::fs::write(&path, &content)?;
            eprintln!("{} Angebote nach '{}' exportiert.", offers.len(), path);
        }
        None => println!("{content}"),
    }
    Ok(())
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn offers_to_csv(offers: &[Offer]) -> String {
    let mut out = String::from(
        "id,market_id,title,subtitle,overline,price,regular_price,category,nutri_score,valid_from,valid_until,images,biozid,flyer_page\n",
    );
    for o in offers {
        let opt = |v: &Option<String>| csv_escape(v.as_deref().unwrap_or(""));
        let num = |v: Option<f64>| v.map(|p| format!("{p:.2}")).unwrap_or_default();
        let row = [
            csv_escape(&o.id),
            csv_escape(&o.market_id),
            csv_escape(&o.title),
            opt(&o.subtitle),
            opt(&o.overline),
            num(o.price),
            num(o.regular_price),
            opt(&o.category),
            opt(&o.nutri_score),
            opt(&o.valid_from),
            opt(&o.valid_until),
            csv_escape(&o.images.join(" ")),
            (o.biozid as i64).to_string(),
            o.flyer_page.map(|p| p.to_string()).unwrap_or_default(),
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

fn format_offer(offer: &Offer) -> String {
    let price_str = offer.price
        .map(|p| format!("{p:.2} €"))
        .unwrap_or_else(|| "-".to_string());
    let reg_str = offer.regular_price
        .map(|p| format!(" (statt {p:.2} €)"))
        .unwrap_or_default();
    let dates = offer.valid_from.as_ref()
        .map(|f| format!("  [{} – {}]", f, offer.valid_until.as_deref().unwrap_or("?")))
        .unwrap_or_default();
    format!(
        "[{cat}] {title}{sub} — {price_str}{reg_str}{dates}",
        cat = offer.category.as_deref().unwrap_or("?"),
        title = offer.title,
        sub = offer.subtitle.as_ref().map(|s| format!(" ({s})")).unwrap_or_default(),
    )
}

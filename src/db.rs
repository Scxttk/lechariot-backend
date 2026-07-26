use anyhow::{Result, bail};
use rusqlite::{Connection, params};

use crate::models::{Market, Offer};

/// Aktuelle Schema-Version (PRAGMA user_version). Zukünftige Schema-Änderungen
/// müssen SCHEMA_VERSION erhöhen und in migrate() einen Migrationsschritt
/// von der Vorversion ergänzen.
pub const SCHEMA_VERSION: i64 = 5;

pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

fn migrate(conn: &Connection) -> Result<()> {
    let mut version = schema_version(conn)?;
    if version > SCHEMA_VERSION {
        bail!(
            "Datenbank hat Schema-Version {version}, dieses Programm unterstützt maximal {SCHEMA_VERSION}. Bitte lechariot aktualisieren."
        );
    }
    while version < SCHEMA_VERSION {
        match version {
            // 0 = neue oder vor Einführung der Versionierung angelegte DB;
            // init_schema ist idempotent (CREATE IF NOT EXISTS) und entspricht v1.
            0 => init_schema(conn)?,
            1 => migrate_v1_to_v2(conn)?,
            2 => migrate_v2_to_v3(conn)?,
            3 => migrate_v3_to_v4(conn)?,
            4 => migrate_v4_to_v5(conn)?,
            v => bail!("Unbekannte Schema-Version {v} — keine Migration definiert."),
        }
        version += 1;
        conn.execute_batch(&format!("PRAGMA user_version = {version};"))?;
    }
    Ok(())
}

// v5: Kette am Markt. Der Push braucht für `offers.market` den kanonischen
// Kettennamen; ohne diese Spalte musste er ihn aus ID und Filialname raten,
// was Filialen mit mehrdeutigem Namen falsch zuordnete ("ALDI SÜD
// Köln-Altstadt-Nord" -> ALDI Nord). Bestandszeilen bleiben NULL und fallen
// in push::chain_for weiter auf die Heuristik zurück.
fn migrate_v4_to_v5(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE markets ADD COLUMN chain TEXT;")?;
    Ok(())
}

// v4: Cache gespiegelter Produktbilder (Supabase Storage). Merkt sich pro
// Händler-Bild-URL die schon hochgeladene Bucket-URL, damit nächtliche Läufe
// bekannte Bilder ohne Netzwerkzugriff überspringen.
fn migrate_v3_to_v4(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS uploaded_images (
            source_url  TEXT PRIMARY KEY,
            public_url  TEXT NOT NULL,
            uploaded_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

/// Schon gespiegelte Bucket-URL für eine Händler-Bild-URL, falls vorhanden.
pub fn cached_image_url(conn: &Connection, source_url: &str) -> Result<Option<String>> {
    let mut stmt =
        conn.prepare("SELECT public_url FROM uploaded_images WHERE source_url = ?1")?;
    let mut rows = stmt.query([source_url])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Zuordnung Händler-URL -> Bucket-URL merken (idempotent).
pub fn cache_image_url(conn: &Connection, source_url: &str, public_url: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO uploaded_images (source_url, public_url) VALUES (?1, ?2)",
        params![source_url, public_url],
    )?;
    Ok(())
}

// v2: Watchlist-Tabelle
fn migrate_v1_to_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS watches (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            query      TEXT NOT NULL,
            max_price  REAL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS markets (
            id   TEXT PRIMARY KEY,
            name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS offers (
            id            TEXT PRIMARY KEY,
            market_id     TEXT NOT NULL,
            title         TEXT NOT NULL,
            subtitle      TEXT,
            overline      TEXT,
            price         REAL,
            regular_price REAL,
            category      TEXT,
            nutri_score   TEXT,
            valid_from    TEXT,
            valid_until   TEXT,
            images        TEXT,
            biozid        INTEGER DEFAULT 0,
            flyer_page    INTEGER,
            fetched_at    TEXT DEFAULT (datetime('now')),
            FOREIGN KEY (market_id) REFERENCES markets(id)
        );

        CREATE TABLE IF NOT EXISTS price_history (
            offer_id   TEXT NOT NULL,
            market_id  TEXT NOT NULL,
            title      TEXT NOT NULL,
            price      REAL,
            seen_at    TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (offer_id, seen_at)
        );

        CREATE INDEX IF NOT EXISTS idx_price_history_title
            ON price_history (title);
    ")?;
    Ok(())
}

// v3: Einkaufsliste
fn migrate_v2_to_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS shopping_list (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            item     TEXT NOT NULL UNIQUE COLLATE NOCASE,
            added_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

pub struct ListItem {
    pub id: i64,
    pub item: String,
    pub added_at: String,
}

/// true, wenn das Item neu aufgenommen wurde (false = stand schon drauf)
pub fn list_add(conn: &Connection, item: &str) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO shopping_list (item) VALUES (?1)",
        params![item],
    )?;
    Ok(n > 0)
}

/// true, wenn ein Item entfernt wurde
pub fn list_remove(conn: &Connection, item: &str) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM shopping_list WHERE item = ?1 COLLATE NOCASE",
        params![item],
    )?;
    Ok(n > 0)
}

pub fn list_items(conn: &Connection) -> Result<Vec<ListItem>> {
    let mut stmt = conn.prepare("SELECT id, item, added_at FROM shopping_list ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        Ok(ListItem { id: row.get(0)?, item: row.get(1)?, added_at: row.get(2)? })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Anzahl entfernter Items
pub fn list_clear(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM shopping_list", [])?)
}

pub struct Watch {
    pub id: i64,
    pub query: String,
    pub max_price: Option<f64>,
    pub created_at: String,
}

pub fn add_watch(conn: &Connection, query: &str, max_price: Option<f64>) -> Result<i64> {
    conn.execute(
        "INSERT INTO watches (query, max_price) VALUES (?1, ?2)",
        params![query, max_price],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn watches(conn: &Connection) -> Result<Vec<Watch>> {
    let mut stmt =
        conn.prepare("SELECT id, query, max_price, created_at FROM watches ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        Ok(Watch {
            id: row.get(0)?,
            query: row.get(1)?,
            max_price: row.get(2)?,
            created_at: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// true, wenn ein Eintrag gelöscht wurde
pub fn remove_watch(conn: &Connection, id: i64) -> Result<bool> {
    let n = conn.execute("DELETE FROM watches WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

/// Angebote, die auf einen Watch passen: Titel ODER Untertitel enthält die
/// Suchanfrage, optional bis max_price.
pub fn watch_hits(conn: &Connection, watch: &Watch) -> Result<Vec<Offer>> {
    let mut stmt = conn.prepare(
        "SELECT id, market_id, title, subtitle, overline, price, regular_price,
                category, nutri_score, valid_from, valid_until, images, biozid, flyer_page
         FROM offers
         WHERE (title LIKE '%' || ?1 || '%' OR subtitle LIKE '%' || ?1 || '%')
           AND (?2 IS NULL OR price <= ?2)
         ORDER BY price ASC",
    )?;
    let rows = stmt.query_map(params![watch.query, watch.max_price], row_to_offer)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn upsert_market(conn: &Connection, market: &Market) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO markets (id, name, chain) VALUES (?1, ?2, ?3)",
        params![market.id, market.name, market.chain],
    )?;
    Ok(())
}

pub fn markets(conn: &Connection) -> Result<Vec<Market>> {
    let mut stmt = conn.prepare("SELECT id, name, chain FROM markets ORDER BY name")?;
    let rows = stmt.query_map([], |row| {
        // Koordinaten liegen nur im Sync-Speicher, nicht in der lokalen DB.
        Ok(Market { id: row.get(0)?, name: row.get(1)?, lat: None, lon: None, chain: row.get(2)? })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub struct PricePoint {
    pub title: String,
    pub market_id: String,
    pub price: Option<f64>,
    pub seen_at: String,
}

pub fn price_history(conn: &Connection, query: &str) -> Result<Vec<PricePoint>> {
    let mut stmt = conn.prepare(
        "SELECT title, market_id, price, seen_at
         FROM price_history
         WHERE title LIKE '%' || ?1 || '%'
         ORDER BY title, seen_at",
    )?;
    let rows = stmt.query_map(params![query], |row| {
        Ok(PricePoint {
            title: row.get(0)?,
            market_id: row.get(1)?,
            price: row.get(2)?,
            seen_at: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub struct PriceDrop {
    pub title: String,
    pub market_id: String,
    pub old_price: f64,
    pub new_price: f64,
    pub old_seen_at: String,
    pub new_seen_at: String,
}

/// Preissenkungen: pro Angebot den letzten Preisverlaufs-Eintrag mit dem
/// vorherigen vergleichen; nur Verbilligungen, größter Preissturz zuerst.
/// since_days begrenzt auf Einträge, deren letzter Stand nicht älter ist.
pub fn price_drops(conn: &Connection, since_days: Option<i64>) -> Result<Vec<PriceDrop>> {
    let mut stmt = conn.prepare(
        "WITH ranked AS (
            SELECT offer_id, market_id, title, price, seen_at,
                   ROW_NUMBER() OVER (PARTITION BY offer_id ORDER BY seen_at DESC) AS rn
            FROM price_history
            WHERE price IS NOT NULL
        )
        SELECT l.title, l.market_id, p.price, l.price, p.seen_at, l.seen_at
        FROM ranked l
        JOIN ranked p ON p.offer_id = l.offer_id AND p.rn = 2
        WHERE l.rn = 1
          AND l.price < p.price
          AND (?1 IS NULL OR l.seen_at >= date('now', '-' || ?1 || ' days'))
        ORDER BY (p.price - l.price) DESC",
    )?;
    let rows = stmt.query_map(params![since_days], |row| {
        Ok(PriceDrop {
            title: row.get(0)?,
            market_id: row.get(1)?,
            old_price: row.get(2)?,
            new_price: row.get(3)?,
            old_seen_at: row.get(4)?,
            new_seen_at: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn search_offers(conn: &Connection, query: &str, max_price: Option<f64>) -> Result<Vec<Offer>> {
    let mut stmt = conn.prepare(
        "SELECT id, market_id, title, subtitle, overline, price, regular_price,
                category, nutri_score, valid_from, valid_until, images, biozid, flyer_page
         FROM offers
         WHERE title LIKE '%' || ?1 || '%'
           AND (?2 IS NULL OR price <= ?2)
         ORDER BY price ASC",
    )?;
    let rows = stmt.query_map(params![query, max_price], |row| {
        let images_json: String = row.get(11)?;
        Ok(Offer {
            id: row.get(0)?,
            market_id: row.get(1)?,
            title: row.get(2)?,
            subtitle: row.get(3)?,
            overline: row.get(4)?,
            price: row.get(5)?,
            regular_price: row.get(6)?,
            category: row.get(7)?,
            nutri_score: row.get(8)?,
            valid_from: row.get(9)?,
            valid_until: row.get(10)?,
            images: serde_json::from_str(&images_json).unwrap_or_default(),
            biozid: row.get::<_, i64>(12)? != 0,
            flyer_page: row.get(13)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

// Wie search_offers, aber Titel UND Untertitel durchsuchen — manche Scraper
// (z. B. Kaufland) speichern die Marke im Titel und das Produkt im Untertitel.
pub fn search_offers_broad(conn: &Connection, query: &str) -> Result<Vec<Offer>> {
    let mut stmt = conn.prepare(
        "SELECT id, market_id, title, subtitle, overline, price, regular_price,
                category, nutri_score, valid_from, valid_until, images, biozid, flyer_page
         FROM offers
         WHERE title LIKE '%' || ?1 || '%' OR subtitle LIKE '%' || ?1 || '%'
         ORDER BY price ASC",
    )?;
    let rows = stmt.query_map(params![query], row_to_offer)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

// Alle Angebote, optional auf einen Suchbegriff (Titel/Untertitel) gefiltert.
pub fn export_offers(conn: &Connection, query: Option<&str>) -> Result<Vec<Offer>> {
    let mut stmt = conn.prepare(
        "SELECT id, market_id, title, subtitle, overline, price, regular_price,
                category, nutri_score, valid_from, valid_until, images, biozid, flyer_page
         FROM offers
         WHERE ?1 IS NULL OR title LIKE '%' || ?1 || '%' OR subtitle LIKE '%' || ?1 || '%'
         ORDER BY market_id, title",
    )?;
    let rows = stmt.query_map(params![query], row_to_offer)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn row_to_offer(row: &rusqlite::Row) -> rusqlite::Result<Offer> {
    let images_json: String = row.get(11)?;
    Ok(Offer {
        id: row.get(0)?,
        market_id: row.get(1)?,
        title: row.get(2)?,
        subtitle: row.get(3)?,
        overline: row.get(4)?,
        price: row.get(5)?,
        regular_price: row.get(6)?,
        category: row.get(7)?,
        nutri_score: row.get(8)?,
        valid_from: row.get(9)?,
        valid_until: row.get(10)?,
        images: serde_json::from_str(&images_json).unwrap_or_default(),
        biozid: row.get::<_, i64>(12)? != 0,
        flyer_page: row.get(13)?,
    })
}

pub struct MarketStats {
    pub market_id: String,
    pub market_name: String,
    pub offer_count: i64,
    pub valid_from_min: Option<String>,
    pub valid_until_max: Option<String>,
    // Durchschnittlicher Rabatt in Prozent über Angebote mit beiden Preisen
    pub avg_discount_pct: Option<f64>,
}

pub fn market_stats(conn: &Connection) -> Result<Vec<MarketStats>> {
    let mut stmt = conn.prepare(
        "SELECT o.market_id, COALESCE(m.name, o.market_id), COUNT(*),
                MIN(o.valid_from), MAX(o.valid_until),
                AVG(CASE WHEN o.price IS NOT NULL AND o.regular_price > 0
                         THEN (1.0 - o.price / o.regular_price) * 100.0 END)
         FROM offers o LEFT JOIN markets m ON m.id = o.market_id
         GROUP BY o.market_id
         ORDER BY COUNT(*) DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(MarketStats {
            market_id: row.get(0)?,
            market_name: row.get(1)?,
            offer_count: row.get(2)?,
            valid_from_min: row.get(3)?,
            valid_until_max: row.get(4)?,
            avg_discount_pct: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub struct DiscountRow {
    pub market_name: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub price: f64,
    pub regular_price: f64,
    pub discount_pct: f64,
}

pub fn top_discounts(conn: &Connection, limit: i64) -> Result<Vec<DiscountRow>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(m.name, o.market_id), o.title, o.subtitle, o.price, o.regular_price,
                (1.0 - o.price / o.regular_price) * 100.0 AS pct
         FROM offers o LEFT JOIN markets m ON m.id = o.market_id
         WHERE o.price IS NOT NULL AND o.regular_price > 0 AND o.price <= o.regular_price
         ORDER BY pct DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(DiscountRow {
            market_name: row.get(0)?,
            title: row.get(1)?,
            subtitle: row.get(2)?,
            price: row.get(3)?,
            regular_price: row.get(4)?,
            discount_pct: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn upsert_offer(conn: &Connection, offer: &Offer) -> Result<()> {
    let images = serde_json::to_string(&offer.images)?;
    conn.execute(
        "INSERT OR REPLACE INTO offers
            (id, market_id, title, subtitle, overline, price, regular_price,
             category, nutri_score, valid_from, valid_until, images, biozid, flyer_page)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![
            offer.id,
            offer.market_id,
            offer.title,
            offer.subtitle,
            offer.overline,
            offer.price,
            offer.regular_price,
            offer.category,
            offer.nutri_score,
            offer.valid_from,
            offer.valid_until,
            images,
            offer.biozid as i64,
            offer.flyer_page,
        ],
    )?;
    // Ein History-Eintrag pro Angebot und Tag; wiederholte Läufe am selben
    // Tag aktualisieren nur den Preis.
    conn.execute(
        "INSERT OR REPLACE INTO price_history (offer_id, market_id, title, price, seen_at)
         VALUES (?1, ?2, ?3, ?4, date('now'))",
        params![offer.id, offer.market_id, offer.title, offer.price],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_chain_survives_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let koeln = Market::new("ALDI_SUED_B330", "ALDI SÜD Köln-Altstadt-Nord")
            .with_chain("ALDI SÜD");
        upsert_market(&conn, &koeln).unwrap();

        let stored = markets(&conn).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].chain.as_deref(), Some("ALDI SÜD"));
    }

    // Bestands-DBs (Schema < 5) haben keine chain-Spalte: die Migration muss sie
    // ergänzen, ohne die vorhandenen Markt-Zeilen zu verlieren.
    #[test]
    fn legacy_markets_migrate_to_current_schema() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        conn.execute(
            "INSERT INTO markets (id, name) VALUES (?1, ?2)",
            params!["m1", "REWE Christian Koehler oHG"],
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
        let stored = markets(&conn).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "REWE Christian Koehler oHG");
        assert_eq!(stored[0].chain, None);
    }
}

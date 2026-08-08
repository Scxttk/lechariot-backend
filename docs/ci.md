# CI und Nightly-Lauf auf GitHub Actions

Unter `.github/workflows/` liegen:

- **`ci.yml`** — läuft bei jedem Push und Pull Request: `cargo check` und
  `cargo test`. Die 7 Live-Scraper-Tests sind `#[ignore]` und werden **nicht**
  ausgeführt (kein Netzwerkzugriff nötig). Rust-/Cargo-Artefakte werden über
  `Swatinem/rust-cache` gecacht.
- **`nightly.yml`** — der Cloud-Ersatz bzw. das Fallback für den lokalen
  launchd-Agenten (`scripts/nightly.sh`): Release-Build, dann
  `sync-regions` — liest alle aktiven Regionen aus der Supabase-Tabelle
  `public.regions` und macht pro PLZ `fetch --all-stores`, meldet die
  gefundenen Filialen nach `public.markets` und pusht die Angebote mit
  `--region <PLZ>`. Schlägt der ganze Sync fehl (Tabelle leer oder
  unerreichbar, alle Regionen gescheitert), greift der **Fallback**: der
  alte Einzel-PLZ-Pfad `fetch --all-stores --zip $LECHARIOT_ZIP` +
  `push --region $LECHARIOT_ZIP`. Die Pipeline geht also nie dunkel.
  Zum Schluss räumt der volle Lauf auf (bei On-Demand-Läufen für eine PLZ
  oder Filiale übersprungen): `prune-images` entfernt verwaiste Bilder aus
  dem Bucket, `prune-history` beschneidet `public.price_history` auf 26
  Wochen. Beide Befehle sind ohne `--execute` reine Dry-Runs. Die Historie
  braucht das, weil sie als einzige Tabelle **nicht** rotiert: Jeder Push
  legt eine weitere Wochen-Zeile je Produkt und Filiale an, und seit
  Migration v13 multipliziert sich das mit jeder angeforderten Filiale.
  Dazwischen steht seit dem 08.08. der **Bild-Wächter**: `audit-images`
  ruft je Kette eine Stichprobe der wirklich gespeicherten `image_url`
  ab. Trägt eine Kette Bilder, von denen **keins** abrufbar ist, schlägt
  der Lauf fehl — genau der Netto-Fall vom 31.07., bei dem jede Zeile eine
  URL trug und das Telefon trotzdem nur Emojis zeigte.
- **`bild-probe.yml`** — dieselbe Messung von Hand (`workflow_dispatch`) und
  automatisch bei jedem Push, der `storage.rs`, `push.rs` oder `audit.rs`
  anfasst. Rein lesend. Gehört in CI, weil `netto-online.de` einem
  Entwicklungsrechner auf jede Anfrage mit 403 antwortet — von dort gemessen
  wäre jedes Ergebnis eine Aussage über den Mac, nicht über die App.
- **`vorschau-probe.yml`** — Messgeschirr für die Folgewoche, nur GET, ohne
  Supabase-Secrets.
- **`branches.yml`** — frischt das Filialverzeichnis `public.branches` auf
  (`branches-sync --from-regions`), sonntags 03:15 UTC. Bewusst **nicht** in
  der Nightly: Filialen ändern sich in Monaten, Angebote wöchentlich; täglich
  mitzulaufen hieße, deren Laufzeit für ein Ergebnis zu verlängern, das 364
  Tage im Jahr dasselbe ist. Voraussetzung ist die einmalige Migration
  `supabase/migration_v12_branches.sql`. Läuft ohne REWE durch, wenn die
  Zertifikats-Secrets fehlen (Warnung, kein Fehler).

## Multi-Region: einmalige Migration

Der Multi-Region-Sync braucht die Migration
[`supabase/migration_v3_multi_region.sql`](../supabase/migration_v3_multi_region.sql)
— **einmal manuell im Supabase SQL-Editor ausführen** (idempotent, kann
gefahrlos wiederholt werden). Sie ergänzt:

- `regions.requested_at` (Anforderungszeitpunkt) und `regions.active`
  (nur aktive Regionen werden gesynct), plus Check-Constraint „PLZ =
  5 Ziffern".
- Policy „Anon insert": die App darf mit dem anon-Key neue Regionen
  **anfordern** (nur INSERT, kein UPDATE/DELETE).
- Tabelle `public.markets` (`chain`, `branch_name`, `market_id`, `plz`,
  `updated_at`): welche Filiale pro Kette+PLZ gefunden wurde. Public read,
  service write.

Das Verzeichnis `supabase/` in diesem Repo ist ab jetzt die kanonische
Heimat des Schemas (Kopien von `schema.sql`, `migration_v2.sql`,
`migration_regions.sql`, `setup_full.sql` aus dem iOS-Repo plus die neue v3).

Pro Lauf werden höchstens 10 Regionen gesynct (`--max-regions`), weitere
werden geloggt und übersprungen; sortiert wird nach `requested_at`
(älteste Anfrage zuerst). Fehler einzelner Regionen brechen den Lauf nicht
ab — der Sync schlägt nur fehl, wenn **alle** Regionen scheitern.

## On-Demand-Scraping: Trigger auf `regions`

Damit eine neu angeforderte PLZ nicht bis zum nächsten Nightly-Lauf wartet,
feuert ein Datenbank-Trigger bei jedem INSERT in `public.regions` einen
asynchronen HTTP-Call (pg_net) an GitHubs `workflow_dispatch`-API für
`nightly.yml`. Einrichtung:

1. **Fine-grained PAT erstellen** (github.com → Settings → Developer
   settings → Fine-grained tokens): nur Repo `Scxttk/lechariot-backend`,
   Permission **Actions: Read and write**, sonst nichts.
2. **PAT im Supabase Vault ablegen** (SQL-Editor):

   ```sql
   select vault.create_secret('<PAT>', 'github_pat');
   ```

3. **Migration ausführen**:
   [`supabase/migration_v4_region_trigger.sql`](../supabase/migration_v4_region_trigger.sql)
   im SQL-Editor ausführen (idempotent). Aktiviert pg_net, legt die
   Funktion `trigger_region_scrape()` und den Trigger `on_region_insert` an.

**Debugging:** pg_net ist asynchron — das Ergebnis des Calls landet erst
in `net._http_response`:

```sql
select * from net._http_response order by id desc limit 5;
```

Erfolg = Status 204. Häufige Fehler: 401 (PAT abgelaufen/falsch),
404 (PAT ohne Zugriff aufs Repo), 403 ohne `User-Agent`-Header (setzt die
Funktion bereits). Fehlt das Vault-Secret oder schlägt der Call fehl,
wird der INSERT trotzdem durchgelassen (nur `raise warning`).

**PAT rotieren:** neues Token erzeugen, dann im SQL-Editor

```sql
select vault.update_secret(
  (select id from vault.secrets where name = 'github_pat'),
  '<neuer PAT>');
```

— oder einfacher: Secret im Dashboard unter „Vault" aktualisieren.
Die Migration muss dafür nicht neu laufen.

## On-Demand-Regionen absichern: Migration v11

Die ursprüngliche v3-Policy „Anon insert" war `with check (true)` — mit dem
**öffentlichen anon-Key** konnte damit jeder (a) beliebige Werte in die
Queue-Steuerspalten `active`/`last_synced`/`requested_at` schreiben und die
Sync-Queue vergiften, und (b) über den SECURITY-DEFINER-Trigger
`trigger_region_scrape` unbegrenzt `workflow_dispatch`-Aufrufe auslösen
(bezahlt vom hinterlegten PAT). [`supabase/migration_v11_region_insert_hardening.sql`](../supabase/migration_v11_region_insert_hardening.sql)
schließt das (**einmal im SQL-Editor ausführen**, idempotent):

- **Spalten-Härtung.** Die Policy „Anon insert" prüft jetzt
  `plz ~ '^[0-9]{5}$' and active is true and last_synced is null and
  requested_at is not null`, und auf Privilegienebene wird das INSERT-Recht
  von anon/authenticated auf **nur die Spalte `plz`** eingeschränkt
  (`revoke insert … ; grant insert (plz) …`). Der legitime App-Pfad
  `insert into regions (plz) values ('12345')` bleibt unverändert: die
  Steuerspalten füllen ihre Defaults (`requested_at` = `now()`,
  `active` = true, `last_synced` = null).
- **Cooldown pro PLZ statt globalem Budget.** Der Trigger feuert einen
  Dispatch pro PLZ höchstens alle 10 Minuten (protokolliert in
  `public.region_dispatch_log`). Das dedupliziert **nur** wiederholte
  Anfragen für **dieselbe** Region — es gibt **kein** geteiltes globales
  Limit, das die Erst-Anfrage einer fremden, frischen PLZ blockieren
  könnte. Eine frische, nicht kürzlich dispatchte PLZ bekommt **immer**
  ihren einen Dispatch.

> **Hinweis:** `revoke execute on function …` stoppt den Trigger **nicht** —
> Postgres prüft das EXECUTE-Recht nur einmal beim `CREATE TRIGGER`. Jedes
> Ratenlimit muss deshalb im Funktionskörper sitzen (dort sitzt der
> Cooldown), nicht über GRANTs.

### Verbleibendes Risiko und empfohlener Folgeschritt

Der PLZ-Cooldown begrenzt zuverlässig die **Wiederholungs-Verstärkung**
(gleiche PLZ immer wieder), **nicht** aber eine **Distinct-PLZ-Flut**: ein
Angreifer mit dem anon-Key kann viele **unterschiedliche** gültige PLZs
einfügen und je einmal einen Dispatch auslösen. Trigger-seitig lässt sich
das **nicht** abwehren, ohne legitime Erst-Anfragen fremder Nutzer zu
unterdrücken — am Trigger liegt keine Aufrufer-Identität vor, weil die App
den **geteilten** anon-Key nutzt. Bewusst wird deshalb **kein** globales
Budget geschaltet (das würde die Confused-Deputy-Flut nur gegen eine
Denial-of-Service gegen legitime Nutzer eintauschen).

**Empfohlener Folgeschritt** (echte strukturelle Lösung): Regionen-Anfragen
über eine **authentifizierte, ratenbegrenzte Edge Function** routen bzw. in
der Policy `auth.role() = 'authenticated'` verlangen und pro Installation
(Auth-Identität) eine Quota erzwingen. Erst mit einer Aufrufer-Identität
lässt sich die Distinct-PLZ-Flut fair drosseln, ohne andere Nutzer zu
treffen.

## On-Demand-Filialen: Trigger auf `branch_requests` (Migration v14)

Seit Migration v13 gehören Angebote der **Filiale**, nicht der Kette in einer
PLZ. Der Anforderungsweg dafür ist derselbe wie bei Regionen, nur eine Ebene
genauer: Die App wählt eine Filiale aus dem Verzeichnis (`public.branches`)
und inserted deren `market_id` in `public.branch_requests`; der Trigger
`on_branch_request_insert` feuert einen `workflow_dispatch` auf `nightly.yml`
mit `inputs.market_id`, und der Lauf macht daraus
`sync-regions --market-id <ID>`.

[`supabase/migration_v14_branch_requests.sql`](../supabase/migration_v14_branch_requests.sql)
bringt alle Absicherungen aus v11 mit — spaltenweises INSERT-Recht (nur
`market_id`), validierender With-Check auf die Steuerspalten, Cooldown pro
`market_id` im Funktionskörper (`public.branch_dispatch_log`) — und **eine
zusätzliche**:

- **Die `market_id` muss im Verzeichnis stehen.** Bei Regionen kam alles
  durch, was fünf Ziffern hat; so wurde `94108` zur aktiven Region, obwohl es
  die PLZ nicht gibt. Eine Filial-ID lässt sich hart prüfen: Entweder ein
  Store-Finder hat sie geliefert und sie steht in `branches`, oder es gibt
  sie nicht. Der With-Check macht genau das per `exists`-Unterabfrage
  (`branches` hat „Public read", der Check greift also auch für anon).

Zwei Details, die leicht zu übersehen sind:

- Der Trigger **steigt aus, wenn `last_synced` gesetzt ist**. Der Sync meldet
  sich am Ende selbst fertig (`branch_requests.last_synced`, siehe
  `sync::mark_branch_synced`), und für eine Filiale, die niemand vorher
  angefordert hatte, ist das ein INSERT — ohne diese Bremse löste jede
  Fertigmeldung einen neuen Lauf aus, der wieder eine Fertigmeldung schreibt.
- Die **Distinct-Flut** ist hier deutlich kleiner als bei Regionen: statt
  ~8.000 gültiger PLZ lässt sich nur anfordern, was im Verzeichnis steht
  (Stand 2026-07-25: 3.115 Zeilen), und das wächst nur dort, wo jemand die
  App benutzt. Der empfohlene Folgeschritt bleibt derselbe wie oben.

## Produktbilder: einmalige Migrationen v5 + v6

Der Push schreibt seit v5 pro Angebot eine Produktbild-URL und spiegelt die
Bilder seit v6 in einen eigenen Storage-Bucket. Beide Migrationen **einmal
manuell im Supabase SQL-Editor ausführen** (idempotent):

1. [`supabase/migration_v5_image_url.sql`](../supabase/migration_v5_image_url.sql)
   — Spalte `offers.image_url` (optional; das Emoji bleibt Fallback).
2. [`supabase/migration_v6_storage_bucket.sql`](../supabase/migration_v6_storage_bucket.sql)
   — öffentlicher Bucket `offer-images` + Public-Read-Policy. Schreiben
   läuft über den Service-Role-Key (umgeht Storage-RLS), es ist keine
   Insert-Policy nötig.

**Seit 2026-07-27 wird standardmäßig nicht mehr gespiegelt:** `image_url`
trägt die Händler-CDN-URL, die App lädt direkt (Hotlinking statt eigener
Kopien). Der Bucket aus v6 bleibt bestehen; Spiegelung ist Opt-in über
`push --mirror-images`. Schlagen einzelne Uploads dabei fehl, behält die
Zeile die Händler-URL, das Log zählt „fehlgeschlagen" — der Push selbst
geht durch.

## Zeitplan

Der Nightly-Lauf startet per Cron um **04:30 UTC**:

- Sommerzeit (MESZ): **06:30** deutscher Zeit
- Winterzeit (MEZ): **05:30** deutscher Zeit

GitHub-Cron kennt keine Zeitzonen, daher verschiebt sich die lokale Startzeit
mit der Zeitumstellung um eine Stunde. Außerdem garantiert GitHub keine
pünktliche Ausführung — Verzögerungen von einigen Minuten bis über einer
Stunde sind normal.

Das Filialverzeichnis (`branches.yml`) läuft sonntags um **03:15 UTC**, also
gut eine Stunde vor dem Angebots-Sync desselben Morgens: Eine Filiale, die
über Nacht neu ins Verzeichnis kommt, kann so am selben Tag Angebote
bekommen.

## Einmalige Einrichtung

Unter **Settings → Secrets and variables → Actions** im Repo:

**Secrets** (Reiter *Secrets*):

| Name | Inhalt |
|---|---|
| `SUPABASE_URL` | Projekt-URL, z. B. `https://xyz.supabase.co` |
| `SUPABASE_SERVICE_KEY` | Service-Role-Key des Supabase-Projekts |

**Variable** (Reiter *Variables*):

| Name | Inhalt |
|---|---|
| `LECHARIOT_ZIP` | Fallback-Postleitzahl, falls der Regionen-Sync scheitert, z. B. `01219` |

Oder per CLI:

```sh
gh secret set SUPABASE_URL
gh secret set SUPABASE_SERVICE_KEY
gh variable set LECHARIOT_ZIP --body 01219
```

## Manuell starten

Über die GitHub-Oberfläche: **Actions → Nightly fetch+push → Run workflow**.
Oder per CLI:

```sh
gh workflow run nightly.yml
gh run watch          # letzten Lauf live verfolgen
```

Das Filialverzeichnis genauso:

```sh
gh workflow run branches.yml                    # alle aktiven Regionen
gh workflow run branches.yml -f area=01219      # nur ein Gebiet
```

## Ergebnis lesen

- **Job-Summary**: Auf der Lauf-Seite (Actions → Lauf anklicken) steht unter
  *Summary* **pro Region** eine Tabelle „Markt / Filiale / Ergebnis" — pro
  Store entweder die Anzahl der Angebote oder `FEHLER: …`, plus die Anzahl
  der Angebote der zuletzt gesyncten Region in der lokalen DB. Lief der
  Fallback, heißt der Block „Zusammenfassung (Fallback PLZ)".
- **Artefakt `nightly-log`**: das vollständige fetch+push-Log, 7 Tage
  aufbewahrt. Auf der Lauf-Seite unter *Artifacts* herunterladen.

## Fehlerverhalten

- **Einzelne Stores dürfen fehlschlagen** — insbesondere REWE, das ohne
  Client-Zertifikat läuft (siehe `docs/rewe-cert.md`) und daher in CI immer
  als `FEHLER` erscheint. Der Job bleibt grün, solange insgesamt Angebote
  ankommen.
- **Einzelne Regionen dürfen fehlschlagen** — der Sync macht mit der
  nächsten Region weiter und schlägt nur fehl, wenn alle scheitern; dann
  greift der Einzel-PLZ-Fallback.
- **Der Job schlägt fehl**, wenn am Ende **0 Angebote** in der DB liegen
  oder auch der Fallback-Push nach Supabase scheitert.

## Bekanntes Risiko: Akamai vs. Runner-IPs

Netto, ALDI Süd und EDEKA werden über system-`curl` geholt, weil Akamai
schlichte reqwest-Clients blockt. GitHub-Runner laufen auf Azure-IP-Ranges,
die Akamai möglicherweise härter blockt als eine private Wohnungs-IP. Wenn
diese Stores **nur auf dem Runner** fehlschlagen (lokal aber laufen), ist das
genau diese IP-Reputation — das ist ein Datenpunkt, kein Bug. In dem Fall
bleibt der lokale launchd-Agent für diese Stores die verlässliche Quelle;
Proxys o. Ä. sind bewusst nicht vorgesehen. Die höfliche Rate-Limitierung der
Scraper gilt unverändert auch in CI.

## Koexistenz mit dem lokalen launchd-Agenten

Beide Pipelines können parallel laufen: der Push nach Supabase ist ein
**Upsert**, doppelte Läufe sind also idempotent und richten keinen Schaden an.
Empfehlung: eine der beiden Seiten abschalten, damit klar ist, welche Quelle
maßgeblich ist —

- **entweder** den launchd-Agenten deaktivieren
  (`scripts/install-launchd.sh --uninstall`, siehe `docs/automation.md`),
- **oder** den `schedule:`-Block in `nightly.yml` auskommentieren und den
  Cloud-Lauf nur manuell (`workflow_dispatch`) nutzen.

Hinweis: der lokale Agent macht zusätzlich `watch check` + ntfy-Benachrichtigung;
das gibt es im Cloud-Lauf (noch) nicht.

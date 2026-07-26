# Rewe: TLS-Client-Zertifikat einrichten

Der Rewe-Scraper (`src/scrapers/rewe.rs`) ruft die Mobile-API nicht selbst auf,
sondern shellt zum [`rewerse`-CLI](https://github.com/ByteSizedMarius/rewerse-engineering)
aus. Die Rewe-Mobile-API verlangt mutual TLS: ein Client-Zertifikat samt
privatem Schlüssel, die einmalig aus der offiziellen Rewe-Android-App (APK)
extrahiert werden. Ohne diese beiden Dateien funktioniert **nur** der
Rewe-Scraper nicht — alle anderen Ketten brauchen keine Authentifizierung.

## Was lechariot erwartet

`lechariot fetch --store rewe` prüft vor jedem Aufruf:

1. Das Binary `rewerse` ist im `PATH`. Fehlermeldung sonst:
   `rewerse CLI nicht gefunden — bitte installieren (siehe README)`
2. Zertifikat und Schlüssel existieren. Standardpfade (relativ zum
   Arbeitsverzeichnis): `cert.pem` und `private.key`, überschreibbar mit
   `--cert <pfad>` und `--key <pfad>`. Fehlermeldungen sonst:
   `Zertifikat nicht gefunden: cert.pem` bzw. `Private Key nicht gefunden: private.key`

Hinweis: `rewerse` selbst nennt seinen Standard `certificate.pem` — lechariot
übergibt die Pfade aber immer explizit per `-cert`/`-key`, daher zählt nur, was
du lechariot mitgibst.

## Schritt 1: rewerse installieren

Mit Go-Toolchain (falls Go fehlt: `brew install go`):

```sh
go install github.com/ByteSizedMarius/rewerse-engineering/cmd@latest
```

**Stolperstein:** `go install …/cmd@latest` legt das Binary als `cmd` ab
(`~/go/bin/cmd`), nicht als `rewerse`. Umbenennen bzw. symlinken und einen Ort
wählen, der auch im nächtlichen launchd-Lauf im `PATH` liegt — `nightly.sh`
nimmt `~/.cargo/bin` und `~/go/bin` in den `PATH` auf:

```sh
mv ~/go/bin/cmd ~/go/bin/rewerse
ln -sf ~/go/bin/rewerse ~/.cargo/bin/rewerse   # optional, für PATH-Fallback
```

Getestet gegen `rewerse` v1.2.0. Alternativ ein Release-Binary von der
GitHub-Releases-Seite laden.

## Schritt 2: Rewe-APK besorgen

Lade die aktuelle Rewe-App als APK von einem APK-Mirror (z. B. APKPure).
Laut Upstream-Doku ist die App-Version egal. Bei `.apkx`/`.xapk`-Bundles
zuerst entpacken und die eigentliche `.apk` herausnehmen.

**Stolperstein:** Der direkte APKPure-Downloadhost (`d.apkpure.com`) antwortet
per curl gern mit `403` (Cloudflare). Der CDN-Spiegel `d.cdnpure.com` liefert die
APK dagegen aus:

```sh
curl -sL -A "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) …" \
  -o rewe.apk "https://d.cdnpure.com/b/APK/de.rewe.app.mobile?version=latest"
file rewe.apk   # -> "Zip archive data" (nicht "HTML document")
```

Notfalls die APK im Browser herunterladen. Die `mtls_prod.pfx` (~2,6 KB) liegt
unter `res/raw/` im Archiv.

## Schritt 3: PFX aus dem APK extrahieren

Ein APK ist ein ZIP-Archiv:

```sh
unzip -j rewe.apk res/raw/mtls_prod.pfx -d .
```

(Alternativ: `.apk` in `.zip` umbenennen und `res/raw/mtls_prod.pfx`
herauskopieren.)

## Schritt 4: PFX in cert.pem + private.key umwandeln

Das PFX ist mit dem (öffentlich bekannten, im Upstream-Repo dokumentierten)
Passwort `NC3hDTstMX9waPPV` geschützt und verwendet die veraltete
RC2-40-CBC-Verschlüsselung. OpenSSL 3.x braucht deshalb das `-legacy`-Flag:

```sh
openssl pkcs12 -in mtls_prod.pfx -clcerts -nokeys -legacy \
    -passin pass:NC3hDTstMX9waPPV -out cert.pem
openssl pkcs12 -in mtls_prod.pfx -nocerts -nodes -legacy \
    -passin pass:NC3hDTstMX9waPPV -out private.key
```

Bei OpenSSL 1.1 das `-legacy`-Flag weglassen. Windows-Nutzer können
alternativ das PowerShell-Skript `docs/rewerse-engineering.ps1` aus dem
Upstream-Repo verwenden (`./rewerse-engineering.ps1 -PfxPath ./mtls_prod.pfx`);
es nutzt .NET-Krypto statt OpenSSL.

Erwartetes Ergebnis: zwei Textdateien im PEM-Format —

- `cert.pem` mit einem `-----BEGIN CERTIFICATE-----`-Block
- `private.key` mit einem `-----BEGIN PRIVATE KEY-----`-Block

Prüfen:

```sh
openssl x509 -in cert.pem -noout -subject -dates
openssl pkey -in private.key -noout && echo "Key OK"
```

## Schritt 5: Mit lechariot verifizieren

Beide Dateien ins Arbeitsverzeichnis legen (oder Pfade per Flag übergeben)
und einen Trockenlauf starten:

```sh
lechariot fetch --store rewe --zip 50667 --dry-run
# oder mit expliziten Pfaden:
lechariot fetch --store rewe --zip 50667 --cert /pfad/cert.pem --key /pfad/private.key --dry-run
```

Erwartete Ausgabe: Marktsuche, gefundener Markt mit ID, dann die Angebotsliste
(`--dry-run` speichert nichts). Schlägt der Abruf mit
`rewerse markets search fehlgeschlagen` fehl, direkt gegentesten:

```sh
rewerse -cert cert.pem -key private.key -json markets search -query 50667
```

**Stolperstein:** `-json` ist in rewerse v1.2.0 ein *globales* Flag und muss
**vor** dem Kommando stehen (`-json markets search …`). Steht es dahinter,
bricht rewerse mit `flag provided but not defined: -json` ab. lechariot
übergibt es bereits an der richtigen Stelle.

**Gültigkeitsdaten:** Die discounts-Antwort von rewerse v1.2.0 enthält nur
noch ein Top-Level `validUntil` (kein Startdatum mehr). lechariot leitet
`valid_from` deshalb als Montag derselben Woche ab (Rewe-Wochenangebote
gelten Mo–Sa); fehlt auch `validUntil`, wird auf die laufende Woche in
Europe/Berlin zurückgefallen.

Liefert das JSON, liegt das Problem bei lechariot; liefert es einen
TLS-Fehler, sind Zertifikat/Schlüssel defekt oder veraltet — dann Extraktion
mit einer aktuellen APK wiederholen.

## Rechtlicher Hinweis

Zertifikat und Schlüssel stammen aus der offiziellen App und identifizieren
die App, nicht dich. Nutze die API maßvoll (die eingebauten lechariot-Abrufe
sind unkritisch) und gib die extrahierten Dateien nicht weiter.

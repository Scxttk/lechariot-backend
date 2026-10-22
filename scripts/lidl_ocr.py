"""Texterkennung für Lidl-Prospektseiten ohne Textebene.

Aufruf: python3 scripts/lidl_ocr.py BILD...
Ausgabe je erkannter Zeile, tabgetrennt: Bildindex, x, y, Breite, Höhe
(relativ zur Bildgröße, 0..1), Strichstärke (relativ zur Bildhöhe), ob die
Schrift heller ist als ihr Grund (0/1), Text. Der Rust-Teil
(`scrapers::lidl_ocr`) macht daraus Wortkoordinaten im Format von
`pdftotext -bbox-layout`.

RapidOCR statt tesseract: Auf dem Prospekt vom 26.10.2026 las tesseract 5.5
(deu, 300 dpi) keinen einzigen von 23 Sternpreisen, RapidOCR 29 von 32 auf
Seiten, die von Hand nachgezählt wurden. Apple Vision las noch etwas besser,
läuft aber nur auf macOS.
"""

import re
import sys

import cv2
import numpy as np
from rapidocr_onnxruntime import RapidOCR


def stroke(gray: np.ndarray, box: list) -> tuple[float, int]:
    """Strichstärke in Pixeln und ob die Schrift hell auf dunklem Grund steht.

    Die Höhe einer OCR-Zeile verrät den Schriftgrad nicht: Produktnamen,
    Fließtext und Banner kommen alle bei ~8,9 pt heraus (Textebene: 12 / 9,2 /
    >13). Die Strichstärke trennt sie: fette Namen 3,1–3,6 px, Fließtext
    2,1–2,4 px bei 2400 px Bildhöhe, mit einer Lücke bei ~2,8 px über alle
    72 Seiten vom 26.10.2026.
    """
    xs = [p[0] for p in box]
    ys = [p[1] for p in box]
    h, w = gray.shape
    x0, x1 = max(0, int(min(xs))), min(w, int(max(xs)) + 1)
    y0, y1 = max(0, int(min(ys))), min(h, int(max(ys)) + 1)
    crop = gray[y0:y1, x0:x1]
    if crop.size < 20:
        return 0.0, 0
    _, binary = cv2.threshold(crop, 0, 255, cv2.THRESH_BINARY + cv2.THRESH_OTSU)
    # Die Schrift ist die kleinere der beiden Flächen.
    dark_share = (binary == 0).mean()
    light = int(dark_share >= 0.5)
    text = ((binary == 255) if light else (binary == 0)).astype(np.uint8)
    if not text.any():
        return 0.0, light
    dist = cv2.distanceTransform(text, cv2.DIST_L2, 3)
    return float(2 * dist[text > 0].mean()), light


# Ab dieser Zeilenhöhe (Anteil der Bildhöhe) steht ein Sternpreis — dieselbe
# Schranke wie `PRICE_LINE_SHARE` in `scrapers::lidl_ocr`.
PRICE_LINE_SHARE = 0.035
# Was als Preis durchgeht: „2.49", „3.69\"", „149.-", „549.", „799".
PRICE = re.compile(r"^\d{1,4}([.,]\d{2}|[.,]-?|-)?(\D{0,3}|\d)$")
# Ein verlesener Preis: führender Punkt („.660", „.66'6") oder Ziffern um
# ein Apostroph („66'6E").
UPSIDE_DOWN = re.compile(r"^[.,]\d|^\d+'\d")


def upright_price(engine: RapidOCR, image: np.ndarray, box: list, text: str) -> str:
    """Einen verlesenen Preis mit Rand noch einmal lesen.

    Auf dem Prospekt vom 26.10.2026 kam 39.99 als „66'6E" heraus, 0.99 als
    „.660", 9.99 als „.66'6" — aus dem knappen Rahmen der Erkennung liest das
    Modell den Preis wie auf dem Kopf. Mit Rand findet es Euro und Cent als
    zwei Kästen („39." und „99"), von links nach rechts zusammengesetzt ist
    das der Preis.
    """
    # 9.99 kommt auch als glattes „666" heraus (19.10.2026, S. 56): Die
    # Verwechslung macht aus jeder 9 eine 6. Echte 6.66 gibt es, deshalb wird
    # nur nachgelesen, nicht verworfen.
    only_sixes = bool(re.fullmatch(r"6{1,3}[.,]?66\W{0,3}", text))
    if not only_sixes and (PRICE.match(text) or not UPSIDE_DOWN.match(text)):
        return text
    xs = [p[0] for p in box]
    ys = [p[1] for p in box]
    h, w = image.shape[:2]
    pad = 6
    crop = image[
        max(0, int(min(ys)) - pad) : min(h, int(max(ys)) + pad),
        max(0, int(min(xs)) - pad) : min(w, int(max(xs)) + pad),
    ]
    if crop.size == 0:
        return text
    # Welcher Rand reicht, schwankt: 0.99 auf S. 63 kam erst mit 60 px, 9.99
    # auf S. 56 (19.10.) erst mit 120 px. Kleine Kästen im Ausschnitt sind
    # Fußnoten („Letzter Preis auf lidl.de") und bleiben draußen.
    tall = 0.3 * crop.shape[0]
    for pad in (30, 60, 90, 120):
        padded = cv2.copyMakeBorder(crop, pad, pad, pad, pad, cv2.BORDER_REPLICATE)
        result, _ = engine(padded, use_cls=False)
        boxes = [r for r in result or [] if max(p[1] for p in r[0]) - min(p[1] for p in r[0]) >= tall]
        again = "".join(r[1] for r in sorted(boxes, key=lambda r: r[0][0][0]))
        found = re.match(r"^(\d{1,3})[.,]?(\d{2})(?!\d)", again)
        if found:
            return f"{found[1]}.{found[2]}"
    return text


def main() -> None:
    engine = RapidOCR()
    for index, path in enumerate(sys.argv[1:]):
        # Als BGR-Array übergeben, nicht als Pfad: Mit dem Pfad lädt
        # rapidocr_onnxruntime 1.2.3 das Bild selbst und las auf S. 41 vom
        # 26.10.2026 zweimal „.66'6" statt 9.99; als Array beide richtig.
        image = cv2.imread(path)
        gray = cv2.cvtColor(image, cv2.COLOR_BGR2GRAY)
        height, width = gray.shape
        result, _ = engine(image)
        for box, text, _score in result or []:
            xs = [p[0] for p in box]
            ys = [p[1] for p in box]
            text = text.replace("\t", " ").replace("\n", " ").strip()
            if not text:
                continue
            if (max(ys) - min(ys)) / height >= PRICE_LINE_SHARE:
                text = upright_price(engine, image, box, text)
            weight, light = stroke(gray, box)
            print(
                f"{index}\t{min(xs) / width:.5f}\t{min(ys) / height:.5f}\t"
                f"{(max(xs) - min(xs)) / width:.5f}\t{(max(ys) - min(ys)) / height:.5f}\t"
                f"{weight / height:.6f}\t{light}\t{text}",
                flush=True,
            )


if __name__ == "__main__":
    main()

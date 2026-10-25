#!/usr/bin/env python3
"""Erzeugt die Python-Referenz dieselbe JSON, die im Repo liegt?

Die Feedback-Runde schreibt `docs/matching-woerterbuch.json` direkt und führt
ihre Änderungen in `docs/matching-woerterbuch-feedback.json` mit, aus der die
Python-Referenz sie liest. Dieses Skript prüft, dass beide Wege beim selben
Wörterbuch ankommen. `warengruppe` bleibt außen vor: das Feld schreibt
`examples/warengruppen.rs`, nicht die Referenz.

Schreibt nichts ins Repo; die Referenz läuft in einer Kopie.
"""
import importlib.util
import json
import os
import shutil
import sys
import tempfile

DOCS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "docs")

with tempfile.TemporaryDirectory() as tmp:
    for name in ("matching-woerterbuch-eval.py", "matching-woerterbuch-feedback.json"):
        shutil.copy(os.path.join(DOCS, name), tmp)
    spec = importlib.util.spec_from_file_location("ev", os.path.join(tmp, "matching-woerterbuch-eval.py"))
    ev = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(ev)
    ev.schreibe_json()
    referenz = json.load(open(os.path.join(tmp, "matching-woerterbuch.json")))

repo = json.load(open(os.path.join(DOCS, "matching-woerterbuch.json")))
for eintrag in repo["begriffe"].values():
    eintrag.pop("warengruppe", None)

abweichend = [k for k in repo if referenz.get(k) != repo[k]]
begriffe = [b for b in repo["begriffe"] if referenz["begriffe"].get(b) != repo["begriffe"][b]]
if abweichend or begriffe:
    print(f"Python-Referenz und JSON weichen ab: Felder {abweichend}, Begriffe {begriffe[:20]}")
    sys.exit(1)
print(f"Python-Referenz erzeugt dieselbe JSON ({len(repo['begriffe'])} Begriffe).")

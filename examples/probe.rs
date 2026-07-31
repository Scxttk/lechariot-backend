//! Reads `title \t subtitle \t category` from stdin, prints the Rust tags.
//! Lets a synthetic case be put to both engines at once — the counterpart to
//! `dump_tags` for cases that are not in the database yet.
use lechariot::matching::match_keys;
use std::io::BufRead;

fn main() {
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let mut f = line.splitn(3, '\t');
        let title = f.next().unwrap_or("");
        let sub = f.next().unwrap_or("");
        let cat = f.next().unwrap_or("");
        let mut k = match_keys(title, Some(sub), Some(cat));
        k.sort();
        println!("{title}\t{sub}\t{cat}\t{}", k.join(","));
    }
}

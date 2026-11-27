use super::{go_quote, unescape_html};
use serde::Deserialize;

#[derive(Deserialize)]
struct Pair {
    #[serde(rename = "in")]
    input: String,
    out: String,
}

fn pairs(text: &str) -> Vec<Pair> {
    serde_json::from_str(text).unwrap()
}

#[test]
fn html_unescape_matches_go() {
    let corpus = pairs(include_str!(
        "../../tests/fixtures/go_text/html_unescape.json"
    ));
    assert!(corpus.len() > 4000);
    for pair in corpus {
        assert_eq!(unescape_html(&pair.input), pair.out, "{:?}", pair.input);
    }
}

#[test]
fn quote_matches_go() {
    let corpus = pairs(include_str!("../../tests/fixtures/go_text/quote.json"));
    assert!(corpus.len() > 30);
    for pair in corpus {
        assert_eq!(go_quote(&pair.input), pair.out, "{:?}", pair.input);
    }
}

use super::*;

fn paths(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| s.to_string()).collect()
}

#[test]
fn ignored_hits_do_not_take_a_rank() {
    let hits = paths(&["en/meeting_notes.txt", "es/notas_reunion.txt"]);
    let expected = paths(&["es/notas_reunion.txt"]);
    assert_eq!(first_match_rank(&hits, &expected, &[]), Some(2));
    let ignore = paths(&["en/meeting_notes.txt"]);
    assert_eq!(first_match_rank(&hits, &expected, &ignore), Some(1));
}

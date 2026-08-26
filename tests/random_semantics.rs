//! $RANDOM semantics mirroring bash 5.2 (verified empirically):
//! - unseeded interpreters draw from OS entropy (sequences differ),
//! - `RANDOM=N` reseeds with the arithmetic value of N,
//! - subshells and command substitutions reseed from entropy and never
//!   disturb the parent sequence.

use rust_bash::RustBashBuilder;

fn run(script: &str) -> (String, String, i32) {
    let mut sh = RustBashBuilder::new().build().unwrap();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

#[test]
fn random_is_numeric_and_in_bash_range() {
    let (out, _, code) = run("echo $RANDOM $RANDOM $RANDOM");
    assert_eq!(code, 0);
    for word in out.split_whitespace() {
        let v: i64 = word.parse().expect("$RANDOM must be numeric");
        assert!((0..=32767).contains(&v), "out of range: {v}");
    }
}

#[test]
fn random_seed_assignment_gives_reproducible_sequence() {
    // RANDOM=1 must produce the same sequence in every interpreter instance.
    let (a, _, _) = run("RANDOM=1; echo $RANDOM $RANDOM $RANDOM");
    let (b, _, _) = run("RANDOM=1; echo $RANDOM $RANDOM $RANDOM");
    assert_eq!(a, b);
}

#[test]
fn random_seed_assignment_evaluates_arithmetic() {
    // bash: RANDOM=abc behaves like RANDOM=0 (unset var → 0); RANDOM=1+2 ≡ RANDOM=3.
    let (abc, _, _) = run("RANDOM=abc; echo $RANDOM $RANDOM");
    let (zero, _, _) = run("RANDOM=0; echo $RANDOM $RANDOM");
    assert_eq!(abc, zero);
    let (expr, _, _) = run("RANDOM=1+2; echo $RANDOM $RANDOM");
    let (three, _, _) = run("RANDOM=3; echo $RANDOM $RANDOM");
    assert_eq!(expr, three);
}

#[test]
fn random_zero_seed_is_not_stuck() {
    // Seed 0 is remapped (xorshift would otherwise lock at 0); sequence
    // must still vary between draws.
    let (out, _, _) = run("RANDOM=0; echo $RANDOM $RANDOM $RANDOM");
    let mut it = out.split_whitespace();
    let (a, b) = (it.next().unwrap(), it.next().unwrap());
    assert_ne!(a, b, "zero seed produced a stuck sequence: {out}");
}

#[test]
fn unseeded_interpreters_diverge() {
    // Two fresh shells draw from OS entropy: five draws each, collision
    // probability is negligible (~2^-75).
    let (a, _, _) = run("echo $RANDOM $RANDOM $RANDOM $RANDOM $RANDOM");
    let (b, _, _) = run("echo $RANDOM $RANDOM $RANDOM $RANDOM $RANDOM");
    assert_ne!(a, b, "two unseeded shells produced the same sequence");
}

#[test]
fn subshell_reseeds_and_parent_sequence_is_unaffected() {
    // bash semantics: a subshell reseeds from entropy, so its first draw
    // differs from the parent's next draw, and the parent sequence
    // continues as if the subshell never happened.
    let (out, _, _) = run("RANDOM=1; p1=$RANDOM; (echo sub=$RANDOM); echo parent-next=$RANDOM");
    let (reference, _, _) = run("RANDOM=1; p1=$RANDOM; echo parent-next=$RANDOM");
    let sub_val = out
        .split_whitespace()
        .next()
        .and_then(|w| w.strip_prefix("sub="))
        .unwrap()
        .to_string();
    let parent_next_actual = out.lines().nth(1).unwrap().to_string();
    let parent_next_expected = reference.lines().next().unwrap().to_string();
    assert_eq!(
        parent_next_actual, parent_next_expected,
        "subshell disturbed the parent sequence"
    );
    // The subshell's draw is entropy-seeded: it must not equal the parent's
    // (seed-1) second draw — collision chance is 1/32768; guard by checking
    // the documented seed-1 sequence slot instead of looping.
    let parent_second = parent_next_expected
        .strip_prefix("parent-next=")
        .unwrap()
        .to_string();
    assert_ne!(
        sub_val, parent_second,
        "subshell continued the parent sequence instead of reseeding"
    );
}

#[test]
fn two_same_seed_subshells_diverge_from_each_other() {
    // Both subshells reseed from entropy, so they differ from each other.
    let (out, _, _) = run("RANDOM=1; (echo a=$RANDOM); (echo b=$RANDOM)");
    let a = out.split_whitespace().next().unwrap().to_string();
    let b = out.split_whitespace().nth(1).unwrap().to_string();
    assert_ne!(a, b, "subshells did not reseed independently: {out}");
}

#[test]
fn command_substitution_reseeds_and_parent_is_unaffected() {
    let (out, _, _) = run("RANDOM=1; p1=$RANDOM; x=$(echo $RANDOM); echo next=$RANDOM");
    let (reference, _, _) = run("RANDOM=1; p1=$RANDOM; echo next=$RANDOM");
    let next_actual = out.lines().next().unwrap();
    let next_expected = reference.lines().next().unwrap();
    assert_eq!(
        next_actual, next_expected,
        "command substitution disturbed the parent sequence"
    );
}

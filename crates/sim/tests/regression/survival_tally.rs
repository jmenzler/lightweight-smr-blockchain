use sim::analysis::SurvivalTally;
use sim::{Availability, Outcome};

fn agreement(rounds: usize, availability: Availability) -> Outcome {
    Outcome::Agreement {
        value: 1,
        rounds,
        availability,
    }
}

#[test]
fn a_collapsing_run_is_censored_not_a_death_at_the_horizon() {
    let mut t = SurvivalTally::default();
    t.add(&agreement(40, Availability::Collapsing { at: 2000 }));
    t.add(&Outcome::NoConvergence { rounds: 2000 });
    assert_eq!(t.death_rounds, Vec::<usize>::new());
    assert_eq!((t.runs, t.survived, t.censored, t.collapsing), (2, 0, 2, 1));
    assert!(
        !t.none_survive(),
        "a cell whose every fate is unknown is no evidence that nothing survives"
    );
}

#[test]
fn deaths_survivals_and_censoring_are_tallied_apart() {
    let mut t = SurvivalTally::default();
    t.add(&agreement(12, Availability::Held));
    t.add(&agreement(30, Availability::Extinct { at: 35 }));
    t.add(&Outcome::AllUndecided { rounds: 50 });
    t.add(&Outcome::NoConvergence { rounds: 2000 });
    assert_eq!(t.agree_rounds, vec![12]);
    assert_eq!(t.death_rounds, vec![35, 50]);
    assert_eq!((t.runs, t.survived, t.censored, t.collapsing), (4, 1, 1, 0));
    assert!(!t.none_survive());

    let mut dead = SurvivalTally::default();
    dead.add(&Outcome::AllUndecided { rounds: 50 });
    dead.add(&Outcome::NoConvergence { rounds: 2000 });
    assert!(dead.none_survive(), "one observed death and no survivor");
}

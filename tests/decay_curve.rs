use phosphor_data::{DecayTerm, classify_decay_terms};

#[test]
fn p1_decay_matches_selomulya() {
    let terms = vec![
        DecayTerm::Exponential {
            amplitude: 6.72,
            tau: 0.00288,
        },
        DecayTerm::Exponential {
            amplitude: 1.0,
            tau: 0.0151,
        },
    ];

    let sum_a: f32 = terms
        .iter()
        .map(|t| match t {
            DecayTerm::Exponential { amplitude, .. } => *amplitude,
            _ => 0.0,
        })
        .sum();

    let at = |t: f32| -> f32 {
        terms
            .iter()
            .map(|term| match term {
                DecayTerm::Exponential { amplitude, tau } => (amplitude / sum_a) * (-t / tau).exp(),
                _ => 0.0,
            })
            .sum::<f32>()
    };

    // At t=0, normalized intensity = 1.0
    assert!((at(0.0) - 1.0).abs() < 0.01);

    // Selomulya: bi-exponential with fast component (tau=2.88ms) dominant.
    // At 8ms (~2.8 fast time constants), most energy has decayed.
    let at_8ms = at(0.008);
    assert!(at_8ms > 0.05 && at_8ms < 0.3, "at 8ms: {at_8ms}");
}

#[test]
fn p31_classification_correct() {
    let terms = vec![
        DecayTerm::PowerLaw {
            amplitude: 2.1e-4,
            alpha: 5.5e-6,
            beta: 1.1,
        },
        DecayTerm::Exponential {
            amplitude: 90.0,
            tau: 31.8e-9,
        },
        DecayTerm::Exponential {
            amplitude: 100.0,
            tau: 227e-9,
        },
        DecayTerm::Exponential {
            amplitude: 37.0,
            tau: 1.06e-6,
        },
    ];
    let class = classify_decay_terms(&terms, 1e-4);
    assert_eq!(class.instant_exp_count, 3);
    assert_eq!(class.slow_exp_count, 0);
    assert!(class.has_power_law);
    assert_eq!(class.accum_layers(), 2); // was 17: now 0 slow + 2 power-law = 2
}

#[test]
fn p31_power_law_long_tail() {
    // ZnS:Cu power-law: I(t) = A / (t + alpha)^beta
    let alpha: f32 = 5.5e-6;
    let beta: f32 = 1.1;

    // At t = 1ms (~180x alpha): still significant
    let at_1ms = (alpha / (0.001 + alpha)).powf(beta);
    assert!(at_1ms > 1e-4, "at 1ms: {at_1ms}");

    // At t = 1s: very small but nonzero
    let at_1s = (alpha / (1.0 + alpha)).powf(beta);
    assert!(at_1s < 1e-5, "at 1s: {at_1s}");
    assert!(at_1s > 0.0, "must be nonzero");
}

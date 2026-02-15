use levenberg_marquardt::{LeastSquaresProblem, LevenbergMarquardt};
use nalgebra::{Matrix3, Owned, U3, Vector3};

struct DecayFitProblem {
    params: Vector3<f64>,
    t: [f64; 3],
    targets: [f64; 3],
}

impl DecayFitProblem {
    fn new(t_10pct: f64, t_1pct: f64, t_01pct: f64) -> Self {
        let tau_f_init = -t_10pct / (0.10_f64).ln();
        let tau_s_init = -t_01pct / (0.001_f64).ln();
        let a_f_init = 0.5_f64;

        Self {
            params: Vector3::new(
                tau_f_init.ln(),
                tau_s_init.ln(),
                (a_f_init / (1.0 - a_f_init)).ln(),
            ),
            t: [t_10pct, t_1pct, t_01pct],
            targets: [0.10, 0.01, 0.001],
        }
    }

    fn decode(&self) -> (f64, f64, f64) {
        let tau_f = self.params[0].exp();
        let tau_s = self.params[1].exp();
        let a_f = 1.0 / (1.0 + (-self.params[2]).exp());
        (tau_f, tau_s, a_f)
    }

    fn decay_at(t: f64, tau_f: f64, tau_s: f64, a_f: f64) -> f64 {
        a_f * (-t / tau_f).exp() + (1.0 - a_f) * (-t / tau_s).exp()
    }
}

impl LeastSquaresProblem<f64, U3, U3> for DecayFitProblem {
    type ParameterStorage = Owned<f64, U3>;
    type ResidualStorage = Owned<f64, U3>;
    type JacobianStorage = Owned<f64, U3, U3>;

    fn set_params(&mut self, p: &Vector3<f64>) {
        self.params.copy_from(p);
    }

    fn params(&self) -> Vector3<f64> {
        self.params
    }

    fn residuals(&self) -> Option<Vector3<f64>> {
        let (tau_f, tau_s, a_f) = self.decode();
        Some(Vector3::new(
            Self::decay_at(self.t[0], tau_f, tau_s, a_f) - self.targets[0],
            Self::decay_at(self.t[1], tau_f, tau_s, a_f) - self.targets[1],
            Self::decay_at(self.t[2], tau_f, tau_s, a_f) - self.targets[2],
        ))
    }

    fn jacobian(&self) -> Option<Matrix3<f64>> {
        let (tau_f, tau_s, a_f) = self.decode();
        let a_s = 1.0 - a_f;
        let sig_deriv = a_f * a_s;

        let mut jac = Matrix3::zeros();
        for (row, &t) in self.t.iter().enumerate() {
            let exp_f = (-t / tau_f).exp();
            let exp_s = (-t / tau_s).exp();
            jac[(row, 0)] = a_f * exp_f * t / tau_f;
            jac[(row, 1)] = a_s * exp_s * t / tau_s;
            jac[(row, 2)] = (exp_f - exp_s) * sig_deriv;
        }
        Some(jac)
    }
}

/// Fit a two-term exponential I(t) = a1*exp(-t/tau1) + a2*exp(-t/tau2)
/// to three decay data points: time to reach 10%, 1%, and 0.1% of initial.
///
/// Returns (tau_fast, tau_slow, a_fast, a_slow) where a_fast + a_slow = 1.0
pub fn fit_decay(t_10pct: f32, t_1pct: f32, t_01pct: f32) -> (f32, f32, f32, f32) {
    let problem = DecayFitProblem::new(t_10pct as f64, t_1pct as f64, t_01pct as f64);
    let (result, _report) = LevenbergMarquardt::new().minimize(problem);
    let (mut tau_f, mut tau_s, mut a_f) = result.decode();

    if tau_f > tau_s {
        std::mem::swap(&mut tau_f, &mut tau_s);
        a_f = 1.0 - a_f;
    }

    (tau_f as f32, tau_s as f32, a_f as f32, (1.0 - a_f) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_decay_matches_p1_data() {
        let (tau_fast, tau_slow, a_fast, a_slow) = fit_decay(0.027, 0.060, 0.095);

        assert!((a_fast + a_slow - 1.0).abs() < 0.001);
        assert!(tau_fast < tau_slow, "fast should be shorter than slow");
        assert!(tau_fast > 0.0);
        assert!(tau_slow > 0.0);

        let i = |t: f32| a_fast * (-t / tau_fast).exp() + a_slow * (-t / tau_slow).exp();
        assert!(
            (i(0.027) - 0.10).abs() < 0.02,
            "10% point: got {}",
            i(0.027)
        );
        assert!(
            (i(0.060) - 0.01).abs() < 0.005,
            "1% point: got {}",
            i(0.060)
        );
        assert!(
            (i(0.095) - 0.001).abs() < 0.002,
            "0.1% point: got {}",
            i(0.095)
        );
    }

    #[test]
    fn fit_decay_matches_p7_data() {
        let (tau_fast, tau_slow, a_fast, a_slow) = fit_decay(0.000305, 0.0057, 0.066);

        let i = |t: f32| a_fast * (-t / tau_fast).exp() + a_slow * (-t / tau_slow).exp();
        assert!((i(0.000305) - 0.10).abs() < 0.02);
        assert!((i(0.0057) - 0.01).abs() < 0.005);
        assert!((i(0.066) - 0.001).abs() < 0.002);
    }
}

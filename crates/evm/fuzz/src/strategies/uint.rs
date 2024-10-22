use alloy_dyn_abi::{DynSolType, DynSolValue};
use alloy_primitives::U256;
use proptest::{
    strategy::{NewTree, Strategy, ValueTree},
    test_runner::TestRunner,
};
use rand::Rng;

/// Value tree for unsigned ints (up to uint256).
pub struct UintValueTree {
    /// Lower base
    lo: U256,
    /// Current value
    curr: U256,
    /// Higher base
    hi: U256,
    /// If true cannot be simplified or complexified
    fixed: bool,
    /// Min Value
    min_bound: U256,
    /// Max Value
    max_bound: U256,
}

impl UintValueTree {
    /// Create a new tree
    /// # Arguments
    /// * `start` - Starting value for the tree
    /// * `fixed` - If `true` the tree would only contain one element and won't be simplified.
    fn new(start: U256, fixed: bool, min_bound: U256, max_bound: U256) -> Self {
        Self { lo: U256::ZERO, curr: start, hi: start, fixed, min_bound, max_bound }
    }

    fn reposition(&mut self) -> bool {
        let interval = self.hi - self.lo;
        let new_mid = self.lo + interval / U256::from(2);

        if new_mid == self.curr {
            false
        } else {
            self.curr = new_mid;
            true
        }
    }
}

impl ValueTree for UintValueTree {
    type Value = U256;

    fn current(&self) -> Self::Value {
        self.curr.clamp(self.min_bound, self.max_bound)
    }

    fn simplify(&mut self) -> bool {
        if self.fixed || (self.hi <= self.lo) {
            return false
        }
        self.hi = self.curr;
        self.reposition()
    }

    fn complicate(&mut self) -> bool {
        if self.fixed || (self.hi <= self.lo) {
            return false
        }

        self.lo = self.curr + U256::from(1);
        self.reposition()
    }
}

/// Value tree for unsigned ints (up to uint256).
/// The strategy combines 3 different strategies, each assigned a specific weight:
/// 1. Generate purely random value in a range. This will first choose bit size uniformly (up `bits`
///    param). Then generate a value for this bit size.
/// 2. Generate a random value around the edges (+/- 3 around 0 and max possible value)
/// 3. Generate a value from a predefined fixtures set
///
/// To define uint fixtures:
/// - return an array of possible values for a parameter named `amount` declare a function `function
///   fixture_amount() public returns (uint32[] memory)`.
/// - use `amount` named parameter in fuzzed test in order to include fixtures in fuzzed values
///   `function testFuzz_uint32(uint32 amount)`.
///
/// If fixture is not a valid uint type then error is raised and random value generated.
#[derive(Debug)]
pub struct UintStrategy {
    /// Bit size of uint (e.g. 256)
    bits: usize,
    /// A set of fixtures to be generated
    fixtures: Vec<DynSolValue>,
    /// The weight for edge cases (+/- 3 around 0 and max possible value)
    edge_weight: usize,
    /// The weight for fixtures
    fixtures_weight: usize,
    /// The weight for purely random values
    random_weight: usize,
    /// Minimum bound for generated values
    min_bound: U256,
    /// Maximum bound for generated values
    max_bound: U256,
    /// Use logarithmic sampling for large ranges
    use_log_sampling: bool,
}

impl UintStrategy {
    /// Create a new strategy.
    /// #Arguments
    /// * `bits` - Size of uint in bits
    /// * `fixtures` - A set of fixed values to be generated (according to fixtures weight)
    pub fn new(
        bits: usize,
        fixtures: Option<&[DynSolValue]>,
        min_bound: Option<U256>,
        max_bound: Option<U256>,
        use_log_sampling: bool,
    ) -> Self {
        let type_max = if bits < 256 { (U256::from(1) << bits) - U256::from(1) } else { U256::MAX };

        let min = min_bound.unwrap_or(U256::ZERO);
        let max = max_bound.unwrap_or(type_max);

        Self {
            bits,
            fixtures: Vec::from(fixtures.unwrap_or_default()),
            edge_weight: 10usize,
            fixtures_weight: 40usize,
            random_weight: 50usize,
            min_bound: min,
            max_bound: max,
            use_log_sampling,
        }
    }

    fn generate_edge_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        let rng = runner.rng();
        // Choose if we want values around 0 or max
        let is_min = rng.gen_bool(0.5);
        let offset = U256::from(rng.gen_range(0..4));
        let start = if is_min {
            self.min_bound.saturating_add(offset)
        } else {
            self.max_bound.saturating_sub(offset)
        };
        Ok(UintValueTree::new(start, false, self.min_bound, self.max_bound))
    }

    fn generate_fixtures_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        // generate random cases if there's no fixtures
        if self.fixtures.is_empty() {
            return self.generate_random_tree(runner)
        }

        // Generate value tree from fixture.
        let fixture = &self.fixtures[runner.rng().gen_range(0..self.fixtures.len())];
        if let Some(uint_fixture) = fixture.as_uint() {
            if uint_fixture.1 == self.bits {
                let fixture_value = uint_fixture.0.clamp(self.min_bound, self.max_bound);
                return Ok(UintValueTree::new(fixture_value, false, self.min_bound, self.max_bound));
            }
        }

        // If fixture is not a valid type, raise error and generate random value.
        error!("{:?} is not a valid {} fixture", fixture, DynSolType::Uint(self.bits));
        self.generate_random_tree(runner)
    }

    fn generate_random_values_uniformly(&self, runner: &mut TestRunner) -> U256 {
        let rng = runner.rng();
        // generate random number of bits uniformly
        let bits = rng.gen_range(0..=self.bits);

        // init 2 128-bit randoms
        let mut higher: u128 = rng.gen_range(0..=u128::MAX);
        let mut lower: u128 = rng.gen_range(0..=u128::MAX);

        // cut 2 randoms according to bits size
        match bits {
            x if x < 128 => {
                lower &= (1u128 << x) - 1;
                higher = 0;
            }
            x if (128..256).contains(&x) => higher &= (1u128 << (x - 128)) - 1,
            _ => {}
        };

        // init U256 from 2 randoms
        let mut inner: [u64; 4] = [0; 4];
        let mask64 = (1 << 65) - 1;
        inner[0] = (lower & mask64) as u64;
        inner[1] = (lower >> 64) as u64;
        inner[2] = (higher & mask64) as u64;
        inner[3] = (higher >> 64) as u64;

        let start: U256 = U256::from_limbs(inner);
        start
    }

    fn generate_random_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        let start = if self.use_log_sampling {
            self.generate_log_uniform(runner)
        } else if self.max_bound > self.min_bound {
            let range = self.max_bound - self.min_bound + U256::from(1);
            if range == U256::ZERO {
                self.min_bound
            } else {
                let random = self.generate_random_values_uniformly(runner) % range;
                self.min_bound + random
            }
        } else {
            self.min_bound
        };

        let clamped_start = start.clamp(self.min_bound, self.max_bound);
        Ok(UintValueTree::new(clamped_start, false, self.min_bound, self.max_bound))
    }

    fn generate_log_uniform(&self, runner: &mut TestRunner) -> U256 {
        if self.max_bound <= self.min_bound {
            return self.min_bound;
        }

        let max_exp = 256;
        let random_exp = runner.rng().gen_range(0..=max_exp);

        let mantissa = U256::from(runner.rng().gen::<u64>());

        let mut value: U256 = (mantissa << random_exp) | (U256::from(1) << random_exp);

        value = value.clamp(self.min_bound, self.max_bound);

        if value == self.min_bound {
            let range = self.max_bound - self.min_bound;
            if range > U256::ZERO {
                value += U256::from(runner.rng().gen::<u64>()) % range;
            }
        }

        value
    }
}

impl Strategy for UintStrategy {
    type Tree = UintValueTree;
    type Value = U256;
    fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        let total_weight = self.random_weight + self.fixtures_weight + self.edge_weight;
        let bias = runner.rng().gen_range(0..total_weight);
        // randomly select one of 3 strategies
        match bias {
            x if x < self.edge_weight => self.generate_edge_tree(runner),
            x if x < self.edge_weight + self.fixtures_weight => self.generate_fixtures_tree(runner),
            _ => self.generate_random_tree(runner),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::strategies::uint::UintValueTree;
    use alloy_dyn_abi::DynSolValue;
    use alloy_primitives::U256;
    use proptest::{prelude::Strategy, strategy::ValueTree, test_runner::TestRunner};

    use super::UintStrategy;

    #[test]
    fn test_uint_tree_complicate_max() {
        let mut uint_tree = UintValueTree::new(U256::MAX, false, U256::MAX, U256::MIN);
        assert_eq!(uint_tree.hi, U256::MAX);
        assert_eq!(uint_tree.curr, U256::MAX);
        uint_tree.complicate();
        assert_eq!(uint_tree.lo, U256::MIN);
    }

    #[test]
    fn test_uint_strategy_respects_bounds() {
        let min = U256::from(1000u64);
        let max = U256::from(2000u64);
        let strategy = UintStrategy::new(16, None, Some(min), Some(max), false);
        let mut runner = TestRunner::default();

        for _ in 0..1000 {
            let value = strategy.new_tree(&mut runner).unwrap().current();
            assert!(value >= min && value <= max, "Generated value {value} is out of bounds");
        }
    }

    #[test]
    fn test_uint_value_tree_bounds() {
        let min = U256::from(100u64);
        let max = U256::from(200u64);
        let start = U256::from(150u64);

        let mut tree = UintValueTree::new(start, false, min, max);

        assert_eq!(tree.current(), start);

        while tree.simplify() {
            let curr = tree.current();
            assert!(curr >= min && curr <= max, "Simplify produced out of bounds value: {curr}");
        }

        tree = UintValueTree::new(start, false, min, max);

        while tree.complicate() {
            let curr = tree.current();
            assert!(curr >= min && curr <= max, "Complicate produced out of bounds value: {curr}");
        }
    }

    #[test]
    fn test_edge_case_generation() {
        let min = U256::from(100u64);
        let max = U256::from(1000u64);
        let strategy = UintStrategy::new(64, None, Some(min), Some(max), false);
        let mut runner = TestRunner::default();

        let mut found_min_area = false;
        let mut found_max_area = false;

        for _ in 0..1000 {
            let tree = strategy.generate_edge_tree(&mut runner).unwrap();
            let value = tree.current();

            assert!(
                value >= min && value <= max,
                "Edge case {value} outside bounds [{min}, {max}]"
            );

            if value <= min + U256::from(3) {
                found_min_area = true;
            }
            if value >= max - U256::from(3) {
                found_max_area = true;
            }
        }

        assert!(found_min_area, "Never generated values near minimum");
        assert!(found_max_area, "Never generated values near maximum");
    }

    #[test]
    fn test_fixture_generation() {
        let min = U256::from(100u64);
        let max = U256::from(1000u64);
        let valid_fixture = U256::from(500u64);
        let fixtures = vec![DynSolValue::Uint(valid_fixture, 64)];

        let strategy = UintStrategy::new(64, Some(&fixtures), Some(min), Some(max), false);
        let mut runner = TestRunner::default();

        for _ in 0..100 {
            let tree = strategy.generate_fixtures_tree(&mut runner).unwrap();
            let value = tree.current();
            assert!(
                value >= min && value <= max,
                "Fixture value {value} outside bounds [{min}, {max}]"
            );
        }
    }

    #[test]
    fn test_log_uniform_sampling() {
        let strategy = UintStrategy::new(256, None, None, None, true);
        let mut runner = TestRunner::default();
        let mut log2_buckets = vec![0; 256];
        let iterations = 100000;

        for _ in 0..iterations {
            let tree = strategy.generate_random_tree(&mut runner).unwrap();
            let value = tree.current();

            // Find the highest set bit (log2 bucket)
            let mut highest_bit = 0;
            for i in 0..256 {
                if value >= (U256::from(1) << i) {
                    highest_bit = i;
                }
            }
            log2_buckets[highest_bit] += 1;
        }

        let mut populated_buckets = 0;
        for &count in &log2_buckets {
            if count > 0 {
                populated_buckets += 1;
            }
        }
        assert!(
            populated_buckets > 200,
            "Log-uniform sampling didn't cover enough orders of magnitude"
        );
    }
}

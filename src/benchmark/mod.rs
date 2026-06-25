pub mod runner;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::{Arc, Mutex};

use crate::backend::{Message, Request};

// Re-export from runner for convenience
pub use runner::Runner;

const CHARS_PER_TOKEN: usize = 4;

static SHAKESPEARE: &str = include_str!("../../data/shakespeare.txt");

pub fn corpus() -> Vec<char> {
    SHAKESPEARE.chars().collect()
}

/// Generates prompts of varying length using a Box-Muller normal distribution,
/// drawn from a pre-generated pool (simulates KV cache hit rate).
///
/// When prompt_stddev > 0:
/// - Pool prompts are generated with max length (prompt_tokens + 3*stddev)
/// - Each request randomly truncates to simulate normal distribution
#[derive(Clone)]
pub struct PromptGenerator {
    prompts: Arc<Vec<String>>,
    idx: Arc<Mutex<usize>>,
    prompt_tokens: usize,
    prompt_stddev: usize,
    rng: Arc<Mutex<StdRng>>,
}

impl PromptGenerator {
    pub fn new(prompt_tokens: usize, seed: u64, prompt_stddev: usize, num_prefix_prompts: usize) -> Self {
        let corpus = corpus();
        let mut gen_rng = StdRng::seed_from_u64(seed);

        // Apply modulo 100 to stddev to keep it in reasonable range (0-99%)
        let effective_stddev = prompt_stddev % 100;

        // Use fixed max length for pool: 2x prompt_tokens (or at least prompt_tokens + 100)
        // This ensures pool prompts are long enough for any truncation,
        // and remain consistent regardless of stddev
        let max_tokens = (prompt_tokens * 2).max(prompt_tokens + 100).min(corpus.len() / CHARS_PER_TOKEN);

        let prompts: Vec<String> = (0..num_prefix_prompts)
            .map(|_| {
                let char_count = max_tokens * CHARS_PER_TOKEN;
                let start = gen_rng.gen_range(0..corpus.len().saturating_sub(char_count).max(1));
                corpus[start..start + char_count].iter().collect()
            })
            .collect();

        // Use random seed for truncation RNG (not based on seed parameter)
        // This ensures each run has different truncation patterns, even with same seed
        let truncation_seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        Self {
            prompts: Arc::new(prompts),
            idx: Arc::new(Mutex::new(0)),
            prompt_tokens,
            prompt_stddev: effective_stddev,
            rng: Arc::new(Mutex::new(StdRng::seed_from_u64(truncation_seed))),
        }
    }

    pub fn next(&self) -> String {
        let mut idx = self.idx.lock().unwrap();
        let full_prompt = self.prompts[*idx % self.prompts.len()].clone();
        *idx += 1;

        // If stddev is 0, return full prompt
        if self.prompt_stddev == 0 {
            return full_prompt;
        }

        // Uniform distribution: prompt_tokens ± stddev%
        let mut rng = self.rng.lock().unwrap();
        let variation = rng.r#gen::<f64>() * 2.0 - 1.0; // -1.0 to 1.0
        let variation_pct = variation * (self.prompt_stddev as f64 / 100.0);
        let target_tokens = (self.prompt_tokens as f64 * (1.0 + variation_pct))
            .max(1.0) as usize;

        let char_count = target_tokens * CHARS_PER_TOKEN;
        let truncate_len = char_count.min(full_prompt.len());

        // Take the first truncate_len characters (prefix)
        full_prompt[..truncate_len].to_string()
    }
}

pub fn new_benchmark_request(model: &str, prompt: &str, max_tokens: usize) -> Request {
    Request {
        model: model.to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: prompt.to_string(),
        }],
        max_tokens,
        temperature: 0.0,
        no_cache: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_generator_non_empty() {
        let prompt_gen = PromptGenerator::new(256, 42, 0, 10);
        let prompt = prompt_gen.next();
        assert!(!prompt.is_empty());
        // Pool prompts are generated at 2x prompt_tokens length
        assert_eq!(prompt.len(), 256 * 2 * CHARS_PER_TOKEN);
    }

    #[test]
    fn test_prompt_generator_pool_cycling() {
        let prompt_gen = PromptGenerator::new(10, 42, 0, 3);
        let p1 = prompt_gen.next();
        let p2 = prompt_gen.next();
        let p3 = prompt_gen.next();
        let p4 = prompt_gen.next(); // should cycle back to p1
        assert_eq!(p1, p4);
        assert_ne!(p1, p2);
    }
}

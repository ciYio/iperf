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
#[derive(Clone)]
pub struct PromptGenerator {
    prompts: Arc<Vec<String>>,
    idx: Arc<Mutex<usize>>,
}

impl PromptGenerator {
    pub fn new(prompt_tokens: usize, seed: u64, prompt_stddev: usize, num_prefix_prompts: usize) -> Self {
        let corpus = corpus();
        let mut gen_rng = StdRng::seed_from_u64(seed);

        let prompts: Vec<String> = (0..num_prefix_prompts)
            .map(|_| {
                let tokens = if prompt_stddev > 0 {
                    // Box-Muller transform for normal distribution
                    let u1: f64 = gen_rng.r#gen::<f64>().max(1e-10);
                    let u2: f64 = gen_rng.r#gen::<f64>();
                    let z = (-2.0_f64 * u1.ln()).sqrt() * (2.0_f64 * std::f64::consts::PI * u2).cos();
                    let t = prompt_tokens as f64 + z * prompt_stddev as f64;
                    (t.max(1.0) as usize).min(corpus.len() / CHARS_PER_TOKEN)
                } else {
                    prompt_tokens
                };
                let char_count = tokens * CHARS_PER_TOKEN;
                let start = gen_rng.gen_range(0..corpus.len().saturating_sub(char_count).max(1));
                corpus[start..start + char_count].iter().collect()
            })
            .collect();

        Self {
            prompts: Arc::new(prompts),
            idx: Arc::new(Mutex::new(0)),
        }
    }

    pub fn next(&self) -> String {
        let mut idx = self.idx.lock().unwrap();
        let prompt = self.prompts[*idx % self.prompts.len()].clone();
        *idx += 1;
        prompt
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
        assert_eq!(prompt.len(), 256 * CHARS_PER_TOKEN);
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

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

        // Pool prompt length:
        // - If stddev=0: use exact prompt_tokens (no truncation needed)
        // - If stddev>0: use 2x prompt_tokens to support variation (up to ±stddev%)
        let max_tokens = if effective_stddev == 0 {
            prompt_tokens.min(corpus.len() / CHARS_PER_TOKEN)
        } else {
            (prompt_tokens * 2).max(prompt_tokens + 100).min(corpus.len() / CHARS_PER_TOKEN)
        };

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
        self.truncate_prompt(&full_prompt)
    }

    /// Get prompt by explicit index (for pool coordination with system prompts).
    /// Does NOT advance internal counter.
    pub fn get(&self, index: usize) -> String {
        let full_prompt = self.prompts[index % self.prompts.len()].clone();
        self.truncate_prompt(&full_prompt)
    }

    fn truncate_prompt(&self, full_prompt: &str) -> String {
        // If stddev is 0, return full prompt
        if self.prompt_stddev == 0 {
            return full_prompt.to_string();
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

/// Generates system prompts with a pool of unique variants.
/// Each variant starts with [NNN] prefix to control prefix cache hit rate.
/// Includes instruction to encourage longer output (controlled by max_tokens).
#[derive(Clone)]
pub struct SystemPromptGenerator {
    prompts: Arc<Vec<String>>,
}

impl SystemPromptGenerator {
    pub fn new(system_prompt_tokens: usize, num_system_prompts: usize, seed: u64) -> Self {
        let corpus = corpus();
        let mut gen_rng = StdRng::seed_from_u64(seed);

        // Instruction to encourage longer output
        let output_instruction = " Please continue writing extensively, as much as possible.";
        let instruction_chars = output_instruction.len();

        let char_count = system_prompt_tokens * CHARS_PER_TOKEN;
        // Reserve space for prefix "[NNN] " (6 chars max for up to 999) and instruction
        let prefix_len = 6;
        let body_len = char_count.saturating_sub(prefix_len + instruction_chars);

        let prompts: Vec<String> = (0..num_system_prompts)
            .map(|i| {
                let start = gen_rng.gen_range(0..corpus.len().saturating_sub(body_len).max(1));
                let body: String = corpus[start..start + body_len].iter().collect();
                format!("[{:03}] {}{}", i + 1, body, output_instruction)
            })
            .collect();

        Self {
            prompts: Arc::new(prompts),
        }
    }

    /// Get system prompt by index (for pool coordination).
    pub fn get(&self, index: usize) -> String {
        self.prompts[index % self.prompts.len()].clone()
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

pub fn new_benchmark_request_with_system(
    model: &str, system_prompt: &str, user_prompt: &str, max_tokens: usize,
) -> Request {
    Request {
        model: model.to_string(),
        messages: vec![
            Message { role: "system".to_string(), content: system_prompt.to_string() },
            Message { role: "user".to_string(), content: user_prompt.to_string() },
        ],
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
        // Pool prompts are generated at exact prompt_tokens length when stddev=0
        assert_eq!(prompt.len(), 256 * CHARS_PER_TOKEN);
    }

    #[test]
    fn test_prompt_generator_pool_cycling() {
        let prompt_gen = PromptGenerator::new(10, 42, 0, 3);
        let p1 = prompt_gen.next();
        let p2 = prompt_gen.next();
        let _p3 = prompt_gen.next();
        let p4 = prompt_gen.next(); // should cycle back to p1
        assert_eq!(p1, p4);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_prompt_generator_get_by_index() {
        let prompt_gen = PromptGenerator::new(10, 42, 0, 3);
        // get() should not advance internal counter
        let p0 = prompt_gen.get(0);
        let p1 = prompt_gen.get(1);
        let p0_again = prompt_gen.get(0);
        assert_eq!(p0, p0_again);
        assert_ne!(p0, p1);
        // get(3) should cycle back to get(0)
        assert_eq!(prompt_gen.get(3), p0);
    }

    #[test]
    fn test_system_prompt_generator_prefix() {
        let sys_gen = SystemPromptGenerator::new(50, 3, 42);
        let p1 = sys_gen.get(0);
        let p2 = sys_gen.get(1);
        let p3 = sys_gen.get(2);
        // Each should start with [NNN] prefix
        assert!(p1.starts_with("[001] "));
        assert!(p2.starts_with("[002] "));
        assert!(p3.starts_with("[003] "));
        // Each should end with output instruction
        assert!(p1.ends_with("Please continue writing extensively, as much as possible."));
        assert!(p2.ends_with("Please continue writing extensively, as much as possible."));
    }

    #[test]
    fn test_system_prompt_generator_pool_cycling() {
        let sys_gen = SystemPromptGenerator::new(50, 3, 42);
        let p1 = sys_gen.get(0);
        let p4 = sys_gen.get(3); // should cycle back
        assert_eq!(p1, p4);
    }

    #[test]
    fn test_system_prompt_generator_single() {
        let sys_gen = SystemPromptGenerator::new(50, 1, 42);
        let p1 = sys_gen.get(0);
        let p2 = sys_gen.get(1);
        // Pool size 1: all return the same prompt
        assert_eq!(p1, p2);
        assert!(p1.starts_with("[001] "));
    }
}

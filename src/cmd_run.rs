use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::backend;
use crate::benchmark::{PromptGenerator, Runner, SystemPromptGenerator};
use crate::cli::RunArgs;
use crate::config::{Config, ConfigOverrides};
use crate::report::Renderer;

pub async fn run(args: RunArgs) -> anyhow::Result<()> {
    // 1. Load config
    let mut cfg = if let Some(ref conf_path) = args.conf {
        Config::load_yaml(Path::new(conf_path))?
    } else {
        Config::default()
    };

    // 2. CLI overrides — only override when explicitly set by user
    let overrides = ConfigOverrides {
        backend: args.backend.clone(),
        base_url: args.target.clone(),
        model: args.model.clone(),
        concurrency: args.concurrency,
        duration: args.duration.as_deref().map(parse_duration).transpose()?,
        request_count: args.request_count,
        mode: args.mode.clone(),
        prompt_tokens: args.prompt_tokens,
        output_tokens: args.output_tokens,
        no_cache: if args.no_cache { Some(true) } else { None },
        num_prefix_prompts: args.num_prefix_prompts,
        cache_rate: args.cache_rate,
        seed: args.seed,
        prompt_tokens_stddev: args.prompt_tokens_stddev,
        system_prompt_tokens: args.system_prompt_tokens,
        num_system_prompts: args.num_system_prompts,
        format: args.format.clone(),
        output_dir: args.output_dir.clone(),
        http_proxy: args.http_proxy.clone(),
        timeout: args.timeout,
        trace: args.trace,
        tag: args.tag.clone(),
    };
    cfg.merge_overrides(&overrides);

    if cfg.model.is_empty() {
        anyhow::bail!("--model is required");
    }

    // Print configuration before running
    eprintln!("Configuration:");
    eprintln!("  model: {}", cfg.model);
    eprintln!("  backend: {}", cfg.backend);
    eprintln!("  base_url: {}", cfg.base_url);
    eprintln!("  concurrency: {}", cfg.concurrency);
    eprintln!("  mode: {}", cfg.mode);
    eprintln!("  prompt_tokens: {}", cfg.prompt_tokens);
    eprintln!("  output_tokens: {}", cfg.output_tokens);
    if cfg.system_prompt_tokens > 0 {
        eprintln!("  system_prompt_tokens: {}", cfg.system_prompt_tokens);
        eprintln!("  num_system_prompts: {}", cfg.num_system_prompts);
    }
    if cfg.duration > 0 {
        eprintln!("  duration: {}s", cfg.duration);
    }
    if cfg.request_count > 0 {
        eprintln!("  request_count: {}", cfg.request_count);
    }
    if cfg.no_cache {
        eprintln!("  no_cache: true");
    }
    if cfg.cache_rate > 0 {
        eprintln!("  cache_rate: {}%", cfg.cache_rate);
    }
    if cfg.seed != 0 {
        eprintln!("  seed: {}", cfg.seed);
    }
    if !cfg.http_proxy.is_empty() {
        eprintln!("  http_proxy: {}", cfg.http_proxy);
    }
    eprintln!("  timeout: {}s", cfg.timeout);
    if !cfg.tag.is_empty() {
        eprintln!("  tag: {}", cfg.tag);
    }
    if args.warmup {
        eprintln!("  warmup: true");
    }
    eprintln!();

    // 3. Create backend
    let timeout = std::time::Duration::from_secs(cfg.timeout);
    let backend_inst = backend::new_backend(&cfg.backend, &cfg.base_url, &cfg.http_proxy, timeout)?;
    let backend_arc: Arc<dyn backend::Backend> = Arc::from(backend_inst);

    // 4. Create prompt generator
    // prompt_tokens = total input (system + user). If system prompt enabled, reduce user tokens.
    let user_prompt_tokens = if cfg.system_prompt_tokens > 0 {
        cfg.prompt_tokens.saturating_sub(cfg.system_prompt_tokens)
    } else {
        cfg.prompt_tokens
    };

    let prompt_gen = PromptGenerator::new(
        user_prompt_tokens,
        cfg.seed as u64,
        cfg.prompt_tokens_stddev,
        cfg.num_prefix_prompts,
    );

    let system_prompt_gen = if cfg.system_prompt_tokens > 0 {
        Some(SystemPromptGenerator::new(
            cfg.system_prompt_tokens,
            cfg.num_system_prompts,
            cfg.seed as u64,
        ))
    } else {
        None
    };

    // 5. Trace mode — output a copy-pasteable curl command and exit
    if let Some(trace_idx) = cfg.trace {
        // trace_idx is 1-based, convert to 0-based for pool indexing
        let idx = trace_idx.saturating_sub(1);
        let is_stream = cfg.mode == "stream";
        let body = if let Some(ref sys_gen) = system_prompt_gen {
            let sys_prompt = sys_gen.get(idx);
            let user_prompt = prompt_gen.get(idx);
            let mut msg = serde_json::json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": sys_prompt},
                    {"role": "user", "content": user_prompt}
                ],
                "max_tokens": cfg.output_tokens,
                "temperature": 0.0,
                "stream": is_stream
            });
            if is_stream {
                msg.as_object_mut().unwrap().insert(
                    "stream_options".to_string(),
                    serde_json::json!({"include_usage": true})
                );
            }
            msg
        } else {
            let user_prompt = prompt_gen.get(idx);
            let min_words = cfg.output_tokens * 5;
            let sys_prompt = format!("Please continue writing. Write at least {} words.", min_words);
            let mut msg = serde_json::json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": sys_prompt},
                    {"role": "user", "content": user_prompt}
                ],
                "max_tokens": cfg.output_tokens,
                "temperature": 0.0,
                "stream": is_stream
            });
            if is_stream {
                msg.as_object_mut().unwrap().insert(
                    "stream_options".to_string(),
                    serde_json::json!({"include_usage": true})
                );
            }
            msg
        };
        let body_str = serde_json::to_string(&body).unwrap();

        let n_flag = if is_stream { "-N " } else { "" };
        let mut curl = format!(
            "curl {}'{}/chat/completions' \\\n  -H 'Content-Type: application/json' \\\n",
            n_flag, cfg.base_url
        );
        if !cfg.http_proxy.is_empty() {
            curl += &format!("  -x {} \\\n", cfg.http_proxy);
        }
        curl += &format!("  -d '{}'", body_str);

        println!("{curl}");
        return Ok(());
    }

    // 6. Set up cancellation token for graceful shutdown
    let cancel = CancellationToken::new();
    let force_cancel = CancellationToken::new();

    // Set up SIGINT handler — first Ctrl+C starts graceful shutdown, second forces immediate exit
    let cancel_for_signal = cancel.clone();
    let force_cancel_for_signal = force_cancel.clone();
    tokio::spawn(async move {
        // First Ctrl+C: start graceful shutdown
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\n  Interrupted! Waiting up to 5s for in-flight requests... (Ctrl+C again to force exit)");
            cancel_for_signal.cancel();

            // Second Ctrl+C: force immediate exit, abort in-flight requests
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\n  Force exit! Aborting in-flight requests...");
                force_cancel_for_signal.cancel();
            }
        }
    });

    // 7. Run benchmark
    if cfg.request_count > 0 {
        eprintln!("Benchmarking {} (backend={}, mode={}, concurrency={}, requests={})",
            cfg.model, cfg.backend, cfg.mode, cfg.concurrency, cfg.request_count);
    } else if cfg.duration > 0 {
        eprintln!("Benchmarking {} (backend={}, mode={}, concurrency={}, duration={}s)",
            cfg.model, cfg.backend, cfg.mode, cfg.concurrency, cfg.duration);
    } else {
        eprintln!("Benchmarking {} (backend={}, mode={}, concurrency={}, until Ctrl+C)",
            cfg.model, cfg.backend, cfg.mode, cfg.concurrency);
    }

    let runner = Runner {
        backend: backend_arc,
        model: cfg.model.clone(),
        concurrent: cfg.concurrency,
        duration: cfg.duration(),
        request_count: cfg.request_count,
        mode: cfg.mode.clone(),
        max_tokens: cfg.output_tokens,
        no_cache: cfg.no_cache,
        cache_rate: cfg.cache_rate,
        system_prompt_gen,
        cancel: cancel.clone(),
        force_cancel: force_cancel.clone(),
    };

    let result = runner.run(&prompt_gen).await?;

    let interrupted = cancel.is_cancelled();

    // 7. Render results
    let renderer = Renderer {
        format: cfg.format.clone(),
        output_dir: cfg.output_dir.clone(),
        model: cfg.model.clone(),
        backend: cfg.backend.clone(),
        base_url: cfg.base_url.clone(),
        concurrent: cfg.concurrency,
        mode: cfg.mode.clone(),
        tag: cfg.tag.clone(),
        prompt_tokens: cfg.prompt_tokens,
        output_tokens: cfg.output_tokens,
        duration: cfg.duration,
        no_cache: cfg.no_cache,
        seed: cfg.seed,
        prompt_tokens_stddev: cfg.prompt_tokens_stddev,
        http_proxy: cfg.http_proxy.clone(),
        cache_rate: cfg.cache_rate,
        num_prefix_prompts: cfg.num_prefix_prompts,
        interrupted,
        warmup: args.warmup,
        show_per_request: args.show_per_request,
    };

    renderer.render(
        &result.stats,
        &result.prefill_decode,
        result.errors,
        result.total_requests,
        result.usage_count,
        &result.samples,
    )?;
    renderer.render_jsonl(
        &result.stats,
        &result.prefill_decode,
        result.errors,
        result.total_requests,
        result.usage_count,
    )?;

    Ok(())
}

pub fn parse_duration(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty duration");
    }

    let (num_str, multiplier) = if let Some(n) = s.strip_suffix('s') {
        (n, 1u64)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3600)
    } else {
        (s, 1u64)
    };

    let num: u64 = num_str.parse()
        .map_err(|_| crate::error::AppError::InvalidDuration(s.to_string()))?;
    Ok(num * multiplier)
}

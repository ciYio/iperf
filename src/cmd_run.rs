use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::backend;
use crate::benchmark::{PromptGenerator, Runner};
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
        format: args.format.clone(),
        output_dir: args.output_dir.clone(),
        http_proxy: args.http_proxy.clone(),
        trace: if args.trace { Some(true) } else { None },
        tag: args.tag.clone(),
    };
    cfg.merge_overrides(&overrides);

    if cfg.model.is_empty() {
        anyhow::bail!("--model is required");
    }

    // 3. Create backend
    let backend_inst = backend::new_backend(&cfg.backend, &cfg.base_url, &cfg.http_proxy)?;
    let backend_arc: Arc<dyn backend::Backend> = Arc::from(backend_inst);

    // 4. Create prompt generator
    let prompt_gen = PromptGenerator::new(
        cfg.prompt_tokens,
        cfg.seed as u64,
        cfg.prompt_tokens_stddev,
        cfg.num_prefix_prompts,
    );

    // 5. Trace mode
    if cfg.trace {
        eprintln!("[trace] Sending test request to {}...", cfg.base_url);
        let prompt = prompt_gen.next();
        let req = crate::benchmark::new_benchmark_request(&cfg.model, &prompt, cfg.output_tokens);
        match backend_arc.send(req).await {
            Ok(resp) => {
                let snippet = &resp.content[..resp.content.len().min(200)];
                eprintln!("[trace] Response: {snippet}...");
            }
            Err(e) => eprintln!("[trace] Error: {e}"),
        }
    }

    // 6. Set up cancellation token for graceful shutdown
    let cancel = CancellationToken::new();

    // Set up SIGINT handler
    let cancel_for_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\n  Interrupted! Finishing current requests and collecting stats...");
            cancel_for_signal.cancel();
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
        trace: cfg.trace,
        cache_rate: cfg.cache_rate,
        cancel: cancel.clone(),
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
        warmup: args.warmup > 0,
    };

    renderer.render(
        &result.stats,
        &result.prefill_decode,
        result.errors,
        result.total_requests,
    )?;
    renderer.render_jsonl(
        &result.stats,
        &result.prefill_decode,
        result.errors,
        result.total_requests,
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

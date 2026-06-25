use crate::watch::types::GpuMetrics;

/// nvidia-smi query fields — order must match FIELD_COUNT
pub const NVIDIA_SMI_QUERY: &str = "index,name,utilization.gpu,utilization.memory,\
    memory.used,memory.total,memory.free,\
    power.draw,power.limit,temperature.gpu,fan.speed.percent,\
    clocks.sm,clocks.memory,pstate,throttle_reasons";

pub const FIELD_COUNT: usize = 15;

pub fn parse_gpu_csv(csv_output: &str) -> anyhow::Result<Vec<GpuMetrics>> {
    let mut gpus = Vec::new();

    for line in csv_output.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }

        let fields = split_nvidia_smi_csv(line);
        if fields.len() < FIELD_COUNT {
            anyhow::bail!("expected {} fields, got {}", FIELD_COUNT, fields.len());
        }

        gpus.push(GpuMetrics {
            gpu_index: parse_u32(&fields[0]).unwrap_or(0),
            gpu_name: fields[1].clone(),
            gpu_utilization: parse_u32(&fields[2]),
            memory_utilization: parse_u32(&fields[3]),
            memory_used: parse_u64(&fields[4]),
            memory_total: parse_u64(&fields[5]),
            memory_free: parse_u64(&fields[6]),
            power_draw: parse_f64(&fields[7]),
            power_limit: parse_f64(&fields[8]),
            temperature_gpu: parse_u32(&fields[9]),
            fan_speed: parse_u32(&fields[10]),
            clock_sm: parse_u32(&fields[11]),
            clock_memory: parse_u32(&fields[12]),
            pstate: Some(fields[13].clone()),
            throttle_reasons: Some(fields[14..].join(", ")),
        });
    }

    if gpus.is_empty() {
        anyhow::bail!("no GPU data parsed from nvidia-smi output");
    }
    Ok(gpus)
}

/// Split nvidia-smi CSV: fields separated by ", ".
/// The last field (throttle_reasons) may contain internal commas.
fn split_nvidia_smi_csv(line: &str) -> Vec<String> {
    let parts: Vec<&str> = line.split(", ").collect();
    if parts.len() <= FIELD_COUNT {
        return parts.iter().map(|s| s.trim().to_string()).collect();
    }
    let mut result: Vec<String> = parts[..FIELD_COUNT - 1]
        .iter()
        .map(|s| s.trim().to_string())
        .collect();
    let throttle = parts[FIELD_COUNT - 1..].join(", ");
    result.push(throttle.trim().to_string());
    result
}

fn parse_u32(s: &str) -> Option<u32> { s.trim().parse().ok() }
fn parse_u64(s: &str) -> Option<u64> { s.trim().parse().ok() }
fn parse_f64(s: &str) -> Option<f64> { s.trim().parse().ok() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_gpu() {
        let csv = "0, NVIDIA A100-SXM4-80GB, 45, 23, 12345, 81920, 69575, 234.5, 400.0, 52, 30, 1200, 1593, P0, none\n";
        let gpus = parse_gpu_csv(csv).unwrap();
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].gpu_index, 0);
        assert_eq!(gpus[0].gpu_name, "NVIDIA A100-SXM4-80GB");
        assert_eq!(gpus[0].gpu_utilization, Some(45));
        assert_eq!(gpus[0].memory_used, Some(12345));
        assert!((gpus[0].power_draw.unwrap() - 234.5).abs() < 0.01);
        assert_eq!(gpus[0].pstate.as_deref(), Some("P0"));
    }

    #[test]
    fn test_parse_multi_gpu() {
        let csv = "\
0, NVIDIA A100, 45, 23, 12345, 81920, 69575, 234.5, 400.0, 52, 30, 1200, 1593, P0, none
1, NVIDIA A100, 67, 45, 23456, 81920, 58464, 289.3, 400.0, 58, 45, 1300, 1600, P0, none
";
        let gpus = parse_gpu_csv(csv).unwrap();
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[1].gpu_index, 1);
        assert_eq!(gpus[1].gpu_utilization, Some(67));
    }

    #[test]
    fn test_parse_throttle_with_commas() {
        let csv = "0, NVIDIA A100, 45, 23, 12345, 81920, 69575, 234.5, 400.0, 52, 30, 1200, 1593, P0, gpu_bw, sw_thermal\n";
        let gpus = parse_gpu_csv(csv).unwrap();
        assert_eq!(gpus[0].throttle_reasons.as_deref(), Some("gpu_bw, sw_thermal"));
    }
}

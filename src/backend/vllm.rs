use super::{openai::OpenAIBackend, Backend};

pub fn register_vllm() {
    super::register("vllm", |base_url| {
        Box::new(OpenAIBackend::new(base_url, "vllm")) as Box<dyn Backend>
    });
}

use super::{openai::OpenAIBackend, Backend};

pub fn register_sglang() {
    super::register("sglang", |base_url| {
        Box::new(OpenAIBackend::new(base_url, "sglang")) as Box<dyn Backend>
    });
}

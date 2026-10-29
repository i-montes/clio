/// Estimates how many model tokens a text costs, so budgets can be enforced without
/// depending on one provider's tokenizer.
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> usize;
}

/// Roughly four characters per token, rounded up. Replace it with a real tokenizer through
/// `Engine::with_token_counter` when exact budgets matter.
pub struct ApproxTokens;

impl TokenCounter for ApproxTokens {
    fn count(&self, text: &str) -> usize {
        text.chars().count().div_ceil(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_tokens_rounds_up() {
        let c = ApproxTokens;
        assert_eq!(c.count(""), 0);
        assert_eq!(c.count("abcd"), 1);
        assert_eq!(c.count("abcde"), 2);
        assert_eq!(c.count("¿qué tal?"), 3, "counts characters, not bytes");
    }
}

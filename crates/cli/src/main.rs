use tracing_subscriber::EnvFilter;

fn main() {
    init_tracing();
    println!("era workspace bootstrap");
}

fn init_tracing() {
    let filter = tracing_filter();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn tracing_filter() -> EnvFilter {
    tracing_filter_from_directive(
        std::env::var("ERA_LOG")
            .or_else(|_| std::env::var("RUST_LOG"))
            .ok(),
    )
}

fn tracing_filter_from_directive(directive: Option<String>) -> EnvFilter {
    directive
        .and_then(|directive| EnvFilter::try_new(directive).ok())
        .unwrap_or_else(|| EnvFilter::new("off"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracing_filter_accepts_valid_directive() {
        let filter = tracing_filter_from_directive(Some("era_object_store=debug".to_owned()));

        assert_eq!(filter.to_string(), "era_object_store=debug");
    }

    #[test]
    fn tracing_filter_defaults_to_off_for_missing_or_invalid_directive() {
        let missing = tracing_filter_from_directive(None);
        let invalid = tracing_filter_from_directive(Some("era_object_store=notalevel".to_owned()));

        assert_eq!(missing.to_string(), "off");
        assert_eq!(invalid.to_string(), "off");
    }
}

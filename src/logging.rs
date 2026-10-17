use log::LevelFilter;
use log4rs::{
    append::console::ConsoleAppender,
    config::{Appender, Config, Logger, Root},
    encode::pattern::PatternEncoder,
};
use std::{env, str::FromStr};

pub(crate) fn init_logging() {
    let rust_log = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let level = LevelFilter::from_str(&rust_log).unwrap_or(LevelFilter::Info);

    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "\x1b[90m[{d}] \x1b[36m{M:<30}\x1b[0m - \x1b[90m{h({l:<5})}\x1b[0m {m}{n}",
        )))
        .build();

    let config = Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .logger(
            Logger::builder()
                .additive(false)
                .build("serenity::gateway", LevelFilter::Warn),
        )
        .logger(
            Logger::builder()
                .additive(false)
                .build("serenity::http::request", LevelFilter::Warn),
        )
        .logger(
            Logger::builder()
                .additive(false)
                .build("serenity::http::client", LevelFilter::Warn),
        )
        .logger(
            Logger::builder()
                .additive(false)
                .build("serenity::http::ratelimiting", LevelFilter::Warn),
        )
        .logger(
            Logger::builder()
                .additive(false)
                .build("tracing", LevelFilter::Warn),
        )
        .build(Root::builder().appender("stdout").build(level))
        .expect("Failed to build logging config");

    log4rs::init_config(config).expect("Failed to init logging");
}

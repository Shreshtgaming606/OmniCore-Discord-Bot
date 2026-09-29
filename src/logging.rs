//! Logging Setup.
//! This uses log and log4rs to create amazing logging

use log::LevelFilter;
use log4rs::{
    append::{console::ConsoleAppender, file::FileAppender},
    config::{Appender, Config, Logger, Root},
    encode::pattern::PatternEncoder,
};
use std::{env, str::FromStr};

/// Initializes logging.
pub(crate) fn init_logging() {
    std::fs::create_dir_all("log").expect("Failed to create log directory");
    // use RUST_log, default to info if none
    let rust_log = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let level = LevelFilter::from_str(&rust_log).unwrap_or(LevelFilter::Info);

    // Console logging pattern:
    // [ {d} ] {M:<30} [{l:<5}] {m}{n}
    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "\x1b[90m[{d}] \x1b[36m{M:<30}\x1b[0m - \x1b[90m{h({l:<5})}\x1b[0m {m}{n}",
        )))
        .build();

    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "[{d}] {M:<30} [{l:<5}] {m}{n}",
        )))
        .build("log/app.log")
        .expect("Failed to create log file");

    let config = Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .appender(Appender::builder().build("files", Box::new(logfile)))
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
        .build(
            Root::builder()
                .appender("stdout")
                .appender("files")
                .build(level),
        )
        .expect("Failed to build logging config");

    log4rs::init_config(config).expect("Failed to init logging");
}

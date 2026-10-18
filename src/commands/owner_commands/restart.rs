//! Owner-only restart for the supervised OmniCore container.

use crate::{CustomContext, Error, commands::ai::imagegen, database};
use poise::CreateReply;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

static RESTART_QUEUED: AtomicBool = AtomicBool::new(false);

/// Restart OmniCore after stopping active image generation and unloading its model.
#[poise::command(
    slash_command,
    guild_only,
    owners_only,
    category = "Bot Owner Utilities"
)]
pub async fn restart(ctx: CustomContext<'_>) -> Result<(), Error> {
    if RESTART_QUEUED.swap(true, Ordering::SeqCst) {
        ctx.send(
            CreateReply::default()
                .content("A restart is already in progress.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    if let Err(error) = ctx
        .send(
            CreateReply::default()
                .content("Restarting OmniCore. Active image requests will be stopped first.")
                .ephemeral(true),
        )
        .await
    {
        RESTART_QUEUED.store(false, Ordering::SeqCst);
        return Err(error.into());
    }
    tokio::spawn(async {
        if tokio::time::timeout(Duration::from_secs(75), imagegen::stop_all_for_restart())
            .await
            .is_err()
        {
            log::error!("Timed out stopping image generation before restart");
        }
        if tokio::time::timeout(Duration::from_secs(10), database::mongo_shutdown())
            .await
            .is_err()
        {
            log::warn!("Timed out closing MongoDB before restart");
        }
        log::info!(
            "Owner-requested restart: exiting for the container supervisor to relaunch OmniCore"
        );
        std::process::exit(0);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_is_owner_only() {
        let command = restart();
        assert!(command.owners_only);
        assert!(command.guild_only);
        assert!(command.slash_action.is_some());
    }
}

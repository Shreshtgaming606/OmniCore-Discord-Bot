use crate::{CustomContext, Error, byok::portal, config::BOT_OWNERS};
use poise::CreateReply;

#[poise::command(slash_command, guild_only, category = "Administration")]
pub async fn management(ctx: CustomContext<'_>) -> Result<(), Error> {
    let guild = ctx.guild_id().ok_or("Guild required")?;
    let user = ctx.author().id;
    let owner = guild.to_partial_guild(ctx.http()).await?.owner_id;
    let bot_owner = BOT_OWNERS
        .get()
        .is_some_and(|owners| owners.contains(&user.get()));
    if user != owner && !bot_owner {
        ctx.send(
            CreateReply::default()
                .content("Only this server's owner or an OmniCore bot owner can open its management page.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let Some(url) = portal::create_management_link(guild.get(), user.get()).await else {
        ctx.send(
            CreateReply::default()
                .content("The secure management page is unavailable. Ask the OmniCore operators to check its configuration.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    ctx.send(
        CreateReply::default()
            .content(format!("Open your private management link within 10 minutes:\n{url}\n\nSign in with the same Discord account. A new /management link replaces your previous link. After you save, this link and its session stop working."))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

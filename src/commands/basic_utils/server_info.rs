use crate::{CustomContext, Error};
use poise::CreateReply;
use poise::serenity_prelude::{ChannelType, Colour, CreateAllowedMentions, CreateEmbed, Timestamp};

#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    description_localized("en-US", "Shows some information about the server."),
    broadcast_typing,
    category = "Utility"
)]
pub async fn serverinfo(ctx: CustomContext<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    // scope so it's safe
    let (
        name,
        owner,
        channels,
        voice_channels,
        roles,
        emojis,
        boosts,
        boost_tier,
        all_members,
        humans,
        bot_members,
        icon_url,
    ) = {
        let guild = ctx.guild().unwrap();

        let owner = guild.owner_id;
        let channels = guild.channels.len();
        let voice_channels = guild
            .channels
            .values()
            .filter(|c| c.kind == ChannelType::Voice || c.kind == ChannelType::Stage)
            .count();
        let roles = guild.roles.len();
        let emojis = guild.emojis.len();
        let boosts = guild.premium_subscription_count.unwrap_or(0);
        let boost_tier = format!("{:?}", guild.premium_tier)
            .to_lowercase()
            .replace("tier", "");
        let all_members = guild.member_count;

        let bot_members = guild.members.values().filter(|m| m.user.bot).count() as u64;
        let humans = all_members.saturating_sub(bot_members);

        let name = guild.name.clone();
        let icon_url = guild.icon_url().unwrap_or_default();

        (
            name,
            owner,
            channels,
            voice_channels,
            roles,
            emojis,
            boosts,
            boost_tier,
            all_members,
            humans,
            bot_members,
            icon_url,
        )
    };

    let text_channels = channels.saturating_sub(voice_channels);
    let res = CreateReply::default()
        .embed(
            CreateEmbed::new()
                .title(name)
                .description(format!(
                    "Owner: <@{}>\nChannels: {}\nText Channels: {}\nVoice Channels: {}\nRoles: {}\nEmojis: {}\nBoosts: {}/14\nBoost Tier: {}/3\nMembers: {} ({} humans, {} bots)\nIcon URL: [Click Here to open]({})",
                    owner, channels, text_channels, voice_channels, roles, emojis, boosts, boost_tier, all_members, humans, bot_members, icon_url
                ))
                .timestamp(Timestamp::now())
                .thumbnail(icon_url)
                .color(Colour::from_rgb(88, 101, 242)),
        )
        .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles())
        .reply(true);

    ctx.send(res).await?;

    Ok(())
}

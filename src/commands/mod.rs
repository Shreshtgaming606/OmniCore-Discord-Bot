//! Shared command modules and helper utilities.
//!
//! This module groups the bot's command categories and exposes small helpers
//! used across multiple commands, such as duration parsing and role
//! comparison.

pub(crate) mod ai;
pub(crate) mod basic_utils;
pub(crate) mod moderation;
pub(crate) mod owner_commands;

use crate::CustomContext;
use poise::serenity_prelude::{Member, Role, RoleId};
use poise::{
    CreateReply,
    serenity_prelude::{Colour, CreateAllowedMentions, CreateEmbed, Timestamp},
};
use regex::Regex;
use std::time::Duration;

/// Builds a standard embed reply used by several commands.
fn build_message_reply(title: &str, desc: &str, color: Colour, mention: bool) -> CreateReply {
    let res = CreateReply::default()
        .embed(
            CreateEmbed::new()
                .description(desc)
                .title(title)
                .timestamp(Timestamp::now())
                .color(color),
        )
        .reply(true);

    if !mention {
        return res.allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles());
    }

    return res;
}

/// Errors returned by [`parse_duration`].
#[derive(Debug)]
pub enum DurationParseError {
    Empty,
    NoMatch,
    Overflow,
}

/// Parses human-readable duration strings like `2h30m` or `1 week`.
pub fn parse_duration(input: &str) -> Result<Duration, DurationParseError> {
    let input = input.trim().to_lowercase();
    if input.is_empty() {
        return Err(DurationParseError::Empty);
    }

    // Matches: number + unit, e.g. "20", "m" / "20minutes" / "2 weeks"
    let re = Regex::new(r"(\d+)\s*([a-z]+)").unwrap();

    let mut total_secs: u64 = 0;
    let mut matched = false;

    for cap in re.captures_iter(&input) {
        matched = true;
        let value: u64 = cap[1].parse().map_err(|_| DurationParseError::Overflow)?;
        let unit = &cap[2];

        let secs_per_unit: u64 = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86400,
            "w" | "wk" | "wks" | "week" | "weeks" => 604800,
            _ => return Err(DurationParseError::NoMatch),
        };

        total_secs = total_secs
            .checked_add(
                value
                    .checked_mul(secs_per_unit)
                    .ok_or(DurationParseError::Overflow)?,
            )
            .ok_or(DurationParseError::Overflow)?;
    }

    if !matched {
        return Err(DurationParseError::NoMatch);
    }

    Ok(Duration::from_secs(total_secs))
}

/// Relative ordering between two Discord roles.
#[derive(Debug)]
pub enum RoleCompareResult {
    Greater,
    Equal,
    Less,
}

/// Compares two roles using Discord's role position ordering.
pub fn compare_roles(role1: &Role, role2: &Role) -> RoleCompareResult {
    let role1_position = role1.position;
    let role2_position = role2.position;

    match role1_position.cmp(&role2_position) {
        std::cmp::Ordering::Greater => RoleCompareResult::Greater,
        std::cmp::Ordering::Equal => RoleCompareResult::Equal,
        std::cmp::Ordering::Less => RoleCompareResult::Less,
    }
}

/// Finds the highest-positioned role currently assigned to a guild member.
pub fn get_highest_role_from_member(member: &Member, ctx: CustomContext<'_>) -> Option<RoleId> {
    let mut highest_role_pos = 0;
    let mut highest_role_id = None;

    for role in &member.roles {
        if let Some(role) = ctx.guild().unwrap().roles.get(&role) {
            if role.position > highest_role_pos {
                highest_role_pos = role.position;
                highest_role_id = Some(role.id);
            }
        }
    }

    highest_role_id
}

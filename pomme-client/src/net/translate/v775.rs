use super::*;

/// Rewrites the game `login` payload: 26.2 added `onlineMode` before the
/// trailing `enforcesSecureChat` bool. It's written false here; the game loop
/// substitutes whether the connection is encrypted, which gated
/// `prepareKeyPair` before 26.2.
pub(super) fn translate_game_login(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let (secure_chat, body) = payload.split_last()?;
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(body);
    out.push(0);
    out.push(*secure_chat);
    Some(out)
}

/// Rewrites `set_player_team` from the pre-26.2 `Parameters` layout
/// (`displayName, options, visibility, collision, color, prefix, suffix`
/// with color as a `ChatFormatting` ordinal) to the 26.2 one
/// (`displayName, prefix, suffix, visibility, collision, color, options`
/// with color as `Optional<TeamColor>`); the surrounding name/method/
/// player-list fields are copied verbatim. `string_scopes` marks the
/// pre-1.21.5 layout where nametag visibility and collision rule are
/// strings rather than enum ids.
pub(super) fn translate_team(id: u32, payload: &[u8], string_scopes: bool) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    skip_utf(&mut cur)?; // team name
    let method_at = cur.position() as usize;
    let method = *payload.get(method_at)?;
    advance(&mut cur, 1)?;

    let mut out = Vec::with_capacity(payload.len() + 3);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..method_at + 1]);

    // Methods 0 (add) and 2 (change) carry Parameters.
    if method == 0 || method == 2 {
        let display = nbt_span(&mut cur)?;
        let options_at = cur.position() as usize;
        let options = *payload.get(options_at)?;
        advance(&mut cur, 1)?;
        let visibility = read_scope(&mut cur, string_scopes)?;
        let collision = read_scope(&mut cur, string_scopes)?;
        let color = var_u32(&mut cur)?;
        let prefix = nbt_span(&mut cur)?;
        let suffix = nbt_span(&mut cur)?;

        out.extend_from_slice(&payload[display]);
        out.extend_from_slice(&payload[prefix]);
        out.extend_from_slice(&payload[suffix]);
        out.push(visibility);
        out.push(collision);
        // Vanilla 26.2 changed color from a ChatFormatting ordinal to
        // Optional<TeamColor>. Pomme owns the native team decoder, so emit
        // the actual 26.2 optional form. The sixteen color ordinals map
        // one-to-one to TeamColor; formatting/reset ordinals become None.
        if color < 16 {
            out.push(1);
            wire::write_varint(&mut out, color);
        } else {
            out.push(0);
        }
        out.push(options);
    }

    // Player list (methods 0/3/4) and anything after: verbatim.
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// One team scope (nametag visibility / collision rule) as its enum id:
/// read directly, or mapped from the pre-1.21.5 name (`Team.Visibility`/
/// `CollisionRule` in the 1.21.5 reference; unknown names fall back to
/// ALWAYS like vanilla's byName).
pub(super) fn read_scope(cur: &mut Cursor<&[u8]>, string_scopes: bool) -> Option<u8> {
    if !string_scopes {
        return Some(var_u32(cur)? as u8);
    }
    let len = var_u32(cur)? as usize;
    let start = cur.position() as usize;
    advance(cur, len)?;
    Some(
        match std::str::from_utf8(&cur.get_ref()[start..start + len]).ok()? {
            "never" => 1,
            "hideForOtherTeams" | "pushOtherTeams" => 2,
            "hideForOwnTeam" | "pushOwnTeam" => 3,
            _ => 0,
        },
    )
}

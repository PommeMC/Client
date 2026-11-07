use super::*;

impl Ids764 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.set_objective_id => translate_set_objective_764,
            i if i == self.resource_pack_push_id => translate_resource_pack_764,
            _ => return None,
        })
    }

    /// Rewrites the JSON components of a packet whose layout is otherwise
    /// unchanged, leaving the rest of the translation chain to run on the
    /// converted payload. Outer `None` = not a component packet;
    /// `Some(None)` = unparsable, drop the frame.
    #[allow(clippy::option_option)]
    pub(super) fn component_pass(&self, id: u32, payload: &[u8]) -> Option<Option<Vec<u8>>> {
        let mut cur = Cursor::new(payload);
        let mut out = Vec::with_capacity(payload.len() + 16);
        let walked = match id {
            i if i == self.system_chat_id
                || i == self.set_action_bar_id
                || i == self.set_title_id
                || i == self.set_subtitle_id
                || i == self.disconnect_id =>
            {
                transcode_component(&mut cur, &mut out)
            }
            i if i == self.tab_list_id => transcode_component(&mut cur, &mut out)
                .and_then(|()| transcode_component(&mut cur, &mut out)),
            i if i == self.open_screen_id => copy_then_transcode(&mut cur, &mut out, 2),
            i if i == self.combat_kill_id => copy_then_transcode(&mut cur, &mut out, 1),
            i if i == self.boss_event_id => transcode_boss_event(&mut cur, &mut out),
            i if i == self.set_player_team_id => transcode_team(&mut cur, &mut out),
            i if i == self.player_chat_id => transcode_player_chat(&mut cur, &mut out),
            i if i == self.disguised_chat_id => transcode_component(&mut cur, &mut out)
                .and_then(|()| transcode_chat_type(&mut cur, &mut out)),
            i if i == self.player_info_id => transcode_player_info(&mut cur, &mut out),
            i if i == self.command_suggestions_id => transcode_suggestions(&mut cur, &mut out),
            _ => return None,
        };
        Some(walked.map(|()| {
            out.extend_from_slice(&payload[cur.position() as usize..]);
            out
        }))
    }
}

/// Rewrites the 1.20.2 `set_score`: the method byte is gone (a REMOVE
/// becomes the `reset_score` packet 1.20.3 added) and the trailing nullable
/// display/numberFormat pair 1.20.3 added is synthesized absent.
pub(super) fn translate_set_score_764(v: &Ids764, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut owner = Vec::new();
    copy_utf(&mut cur, &mut owner)?;
    let method = var_u32(&mut cur)?;
    let mut objective = Vec::new();
    copy_utf(&mut cur, &mut objective)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    if method == 1 {
        // An empty objective resets the owner's scores in every objective.
        wire::write_varint(&mut out, v.reset_score_id);
        out.extend_from_slice(&owner);
        if objective == [0] {
            out.push(0);
        } else {
            out.push(1);
            out.extend_from_slice(&objective);
        }
        return Some(out);
    }
    wire::write_varint(&mut out, v.set_score_id);
    out.extend_from_slice(&owner);
    out.extend_from_slice(&objective);
    copy_varint(&mut cur, &mut out)?; // score
    out.extend_from_slice(&[0, 0]); // no display, no number format
    Some(out)
}

/// Rewrites the 1.20.2 `set_objective`, which ends at the render type; the
/// nullable numberFormat 1.20.3 appended is synthesized absent and the
/// display name transcodes.
pub(super) fn translate_set_objective_764(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    copy_utf(&mut cur, &mut out)?; // objective name
    let method = read_u8(&mut cur)?;
    out.push(method);
    if matches!(method, 0 | 2) {
        transcode_component(&mut cur, &mut out)?;
        copy_varint(&mut cur, &mut out)?; // render type
        // Vanilla 26.2 wraps the numberFormat optional; azalea (ffedf17)
        // reads a bare format kind. Zero decodes as absent/blank either way.
        out.push(0);
    }
    Some(out)
}

/// Rewrites the unsplit 1.20.2 `resource_pack` into `resource_pack_push`:
/// a zero pack UUID is synthesized (the serverbound reply strips it) and
/// the nullable prompt transcodes. Shared by the game and config phases.
pub(super) fn translate_resource_pack_764(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 18);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&[0; 16]); // synthesized pack uuid
    copy_utf(&mut cur, &mut out)?; // url
    copy_utf(&mut cur, &mut out)?; // hash
    copy_bytes(&mut cur, &mut out, 1)?; // required
    copy_optional(&mut cur, &mut out, transcode_component)?; // prompt
    Some(out)
}

/// Rewrites the serverbound `resource_pack` reply for 1.20.2: the pack
/// UUID 1.20.3 prepended is stripped and the post-1.20.2 action values
/// clamp to the original four (downloaded -> accepted, failures -> failed
/// download, discarded -> declined).
pub(super) fn translate_resource_pack_response_764(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        advance(&mut cur, 16)?; // pack uuid
        let action = var_u32(&mut cur)?;
        let action = match action {
            0..=3 => action,
            4 => 3,
            5 | 6 => 2,
            _ => 1,
        };
        let mut out = Vec::with_capacity(4);
        wire::write_varint(&mut out, old_id);
        wire::write_varint(&mut out, action);
        Some(out)
    };
    match rewrite() {
        Some(out) => vec![out],
        None => Vec::new(),
    }
}

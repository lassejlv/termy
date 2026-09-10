use super::*;
use crate::{
    GraphicsAnimation, GraphicsAnimationControl, GraphicsComposition, GraphicsFrameUpdate,
};
use std::time::Instant;

impl GraphicsState {
    pub(super) fn edit_animation(
        &mut self,
        command: &GraphicsCommand,
        patch: Option<crate::GraphicsImage>,
    ) -> Result<u32, String> {
        let id = self
            .resolve_image_id(command)
            .ok_or("ENOENT:image not found")?;
        let image = self.images.get(&id).ok_or("ENOENT:image not found")?;
        let old_bytes = image.byte_len();
        let mut animation = image
            .animation
            .clone()
            .unwrap_or_else(|| GraphicsAnimation::new(image.image.clone(), image.generation));
        let generation = self.next_generation.wrapping_add(1).max(1);
        let value = |key| command.u32_value(key).unwrap_or(0);
        let now = Instant::now();
        match command.char_value('a') {
            Some('f') => animation.update_frame(
                &patch.ok_or("EINVAL:missing frame data")?,
                GraphicsFrameUpdate {
                    target: value('r'),
                    base: value('c'),
                    x: value('x'),
                    y: value('y'),
                    background: value('Y'),
                    replace: value('X') == 1,
                    gap: command.i32_value('z').unwrap_or(0),
                },
                generation,
                now,
            )?,
            Some('a') => animation.control(
                GraphicsAnimationControl {
                    state: value('s'),
                    current: value('c'),
                    frame: value('r'),
                    gap: command.i32_value('z').unwrap_or(0),
                    loops: value('v'),
                },
                now,
            )?,
            Some('c') => animation.compose(
                GraphicsComposition {
                    source: value('r'),
                    destination: value('c'),
                    source_x: value('X'),
                    source_y: value('Y'),
                    destination_x: value('x'),
                    destination_y: value('y'),
                    width: value('w'),
                    height: value('h'),
                    replace: value('C') == 1,
                },
                generation,
            )?,
            Some('d') => {
                if animation.delete_frame(value('r'), now)? {
                    if command.char_value('d') == Some('F') {
                        self.remove_image(id);
                    }
                    return Ok(id);
                }
            }
            _ => return Err("EINVAL:unsupported animation action".into()),
        }
        let bytes = animation.byte_len();
        let evictions = self
            .quota_evictions_for_replacement(id, bytes)
            .ok_or("ENOSPC:animation storage quota exceeded")?;
        for evicted in evictions {
            self.remove_image(evicted);
        }
        let image = self.images.get_mut(&id).ok_or("ENOENT:image not found")?;
        image.image = animation.current_frame().image.clone();
        image.generation = animation.current_frame().generation;
        image.animation = Some(animation);
        self.next_generation = generation;
        self.stored_bytes = self
            .stored_bytes
            .saturating_sub(old_bytes)
            .saturating_add(bytes);
        Ok(id)
    }

    pub(crate) fn advance_animations(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for image in self.images.values_mut() {
            if let Some(animation) = image.animation.as_mut()
                && animation.advance(now)
            {
                image.image = animation.current_frame().image.clone();
                image.generation = animation.current_frame().generation;
                changed = true;
            }
        }
        changed
    }
}

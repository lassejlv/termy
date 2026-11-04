use super::*;
use crate::terminal_engine::media::{
    GraphicsAnimation, GraphicsAnimationControl, GraphicsComposition, GraphicsFrameUpdate,
};
use web_time::Instant;

impl KittyGraphicsState {
    pub(super) fn edit_animation(
        &mut self,
        command: &KittyGraphicsCommand,
        patch: Option<crate::terminal_engine::media::GraphicsImage>,
        viewport: Viewport,
    ) -> Result<u32, String> {
        let id = self
            .resolve_image_id(command)
            .ok_or("ENOENT:image not found")?;
        if command.char_value('a') == Some('c') {
            return self.compose_animation(id, command).map(|()| id);
        }
        let image = self.images.get(&id).ok_or("ENOENT:image not found")?;
        let old_bytes = image.byte_len();
        let mut animation = image
            .animation
            .clone()
            .unwrap_or_else(|| GraphicsAnimation::new(image.image.clone(), image.generation));
        let generation = self.next_generation.wrapping_add(1).max(1);
        let value = |key| command.u32_value(key).unwrap_or(0);
        let now = Instant::now();
        let frame_pixels = image.width as usize * image.height as usize;
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
        if command.char_value('a') == Some('f') {
            self.animation_work = self.animation_work.saturating_add(frame_pixels);
        }
        let bytes = animation.byte_len();
        let evictions = self
            .quota_evictions_for_replacement(id, bytes, viewport)
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

    /// `a=c` keeps the frame size, so it can't change the quota. Unlike
    /// other edits it writes into the destination frame in place, so the
    /// stored handles to it are released first rather than cloned.
    fn compose_animation(&mut self, id: u32, command: &KittyGraphicsCommand) -> Result<(), String> {
        let value = |key| command.u32_value(key).unwrap_or(0);
        let generation = self.next_generation.wrapping_add(1).max(1);
        let mut stored = self.images.remove(&id).ok_or("ENOENT:image not found")?;
        let animated = stored.animation.is_some();
        let mut animation = stored
            .animation
            .take()
            .unwrap_or_else(|| GraphicsAnimation::new(stored.image.clone(), stored.generation));
        // The stored handle would force a copy when the destination is the
        // current frame.
        drop(stored.image);
        let result = animation.compose(
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
        );
        stored.image = animation.current_frame().image.clone();
        stored.generation = animation.current_frame().generation;
        if result.is_ok() || animated {
            stored.animation = Some(animation);
        }
        let (width, height) = (stored.width, stored.height);
        self.images.insert(id, stored);
        result?;
        self.next_generation = generation;
        let w = if value('w') == 0 { width } else { value('w') };
        let h = if value('h') == 0 { height } else { value('h') };
        self.animation_work = self.animation_work.saturating_add(w as usize * h as usize);
        Ok(())
    }

    /// Pixels written by animation edits since the last reset. The engine
    /// pauses its parser once a feed has done enough, so floods of `a=c`/`a=f`
    /// can't hold the engine lock for seconds.
    pub(crate) fn animation_work(&self) -> usize {
        self.animation_work
    }

    pub(crate) fn reset_animation_work(&mut self) {
        self.animation_work = 0;
    }

    pub fn advance_animations(&mut self, now: Instant) -> bool {
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

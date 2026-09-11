use crate::GraphicsImage;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct GraphicsFrame {
    pub image: Arc<GraphicsImage>,
    pub generation: u64,
    pub gap_ms: u32,
}

/// Renderer-independent Kitty animation state. Deadlines are monotonic, and
/// immutable frame generations let a renderer reuse textures across loops.
#[derive(Clone, Debug)]
pub struct GraphicsAnimation {
    pub frames: Vec<GraphicsFrame>,
    pub current: usize,
    state: u32,
    loops: Option<u32>,
    completed_loops: u32,
    deadline: Option<Instant>,
}

#[derive(Default)]
pub struct GraphicsFrameUpdate {
    pub target: u32,
    pub base: u32,
    pub x: u32,
    pub y: u32,
    pub background: u32,
    pub replace: bool,
    pub gap: i32,
}

#[derive(Default)]
pub struct GraphicsAnimationControl {
    pub state: u32,
    pub current: u32,
    pub frame: u32,
    pub gap: i32,
    pub loops: u32,
}

pub struct GraphicsComposition {
    pub source: u32,
    pub destination: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub destination_x: u32,
    pub destination_y: u32,
    pub width: u32,
    pub height: u32,
    pub replace: bool,
}

impl GraphicsAnimation {
    pub fn new(image: Arc<GraphicsImage>, generation: u64) -> Self {
        Self {
            frames: vec![GraphicsFrame {
                image,
                generation,
                gap_ms: 0,
            }],
            current: 0,
            state: 1,
            loops: None,
            completed_loops: 0,
            deadline: None,
        }
    }

    pub fn byte_len(&self) -> usize {
        self.frames.iter().map(|frame| frame.image.byte_len()).sum()
    }
    pub fn current_frame(&self) -> &GraphicsFrame {
        &self.frames[self.current]
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadline
    }

    fn index(&self, number: u32) -> Result<usize, String> {
        number
            .checked_sub(1)
            .map(|n| n as usize)
            .filter(|index| *index < self.frames.len())
            .ok_or_else(|| "ENOENT:animation frame not found".into())
    }

    pub fn update_frame(
        &mut self,
        patch: &GraphicsImage,
        update: GraphicsFrameUpdate,
        generation: u64,
        now: Instant,
    ) -> Result<(), String> {
        if self.frames.len() >= 4096 && update.target == 0 {
            return Err("ENOSPC:too many animation frames".into());
        }
        let root = &self.frames[0].image;
        let (width, height) = (root.width, root.height);
        check_rect(width, height, update.x, update.y, patch.width, patch.height)?;
        let target = if update.target > 0 {
            Some(self.index(update.target)?)
        } else {
            None
        };
        let base = if let Some(target) = target {
            Some(target)
        } else if update.base > 0 {
            Some(self.index(update.base)?)
        } else {
            None
        };
        let mut pixels = if let Some(base) = base {
            self.frames[base]
                .image
                .rgba()
                .ok_or("EINVAL:animation frame has no decoded pixels")?
                .to_vec()
        } else {
            let color = update.background.to_be_bytes();
            (0..u64::from(width) * u64::from(height))
                .flat_map(|_| color)
                .collect()
        };
        let source = patch
            .rgba()
            .ok_or("EINVAL:animation patch has no decoded pixels")?;
        composite(
            &mut pixels,
            width,
            source,
            patch.width,
            (0, 0),
            (update.x, update.y),
            (patch.width, patch.height),
            update.replace,
        );
        let gap_ms = if update.gap == 0 {
            target.map_or(40, |index| self.frames[index].gap_ms)
        } else {
            update.gap.max(0) as u32
        };
        let frame = GraphicsFrame {
            image: Arc::new(GraphicsImage::from_rgba(width, height, pixels)),
            generation,
            gap_ms,
        };
        if let Some(target) = target {
            self.frames[target] = frame;
        } else {
            self.frames.push(frame);
        }
        if self.state != 1 && self.deadline.is_none() {
            self.deadline = Some(now);
        }
        self.advance(now);
        Ok(())
    }

    pub fn compose(&mut self, command: GraphicsComposition, generation: u64) -> Result<(), String> {
        let source = self.index(command.source)?;
        let destination = self.index(command.destination)?;
        let image = &self.frames[source].image;
        let (width, height) = (image.width, image.height);
        let (w, h) = (
            if command.width == 0 {
                width
            } else {
                command.width
            },
            if command.height == 0 {
                height
            } else {
                command.height
            },
        );
        check_rect(width, height, command.source_x, command.source_y, w, h)?;
        check_rect(
            width,
            height,
            command.destination_x,
            command.destination_y,
            w,
            h,
        )?;
        if source == destination
            && command.source_x < command.destination_x + w
            && command.destination_x < command.source_x + w
            && command.source_y < command.destination_y + h
            && command.destination_y < command.source_y + h
        {
            return Err("EINVAL:overlapping composition in the same frame".into());
        }
        let mut pixels = self.frames[destination]
            .image
            .rgba()
            .ok_or("EINVAL:undecoded destination frame")?
            .to_vec();
        composite(
            &mut pixels,
            width,
            image.rgba().ok_or("EINVAL:undecoded source frame")?,
            width,
            (command.source_x, command.source_y),
            (command.destination_x, command.destination_y),
            (w, h),
            command.replace,
        );
        self.frames[destination].image = Arc::new(GraphicsImage::from_rgba(width, height, pixels));
        self.frames[destination].generation = generation;
        Ok(())
    }

    pub fn control(
        &mut self,
        command: GraphicsAnimationControl,
        now: Instant,
    ) -> Result<(), String> {
        if command.state > 3 {
            return Err("EINVAL:invalid animation state".into());
        }
        let current = if command.current > 0 {
            Some(self.index(command.current)?)
        } else {
            None
        };
        let frame = if command.gap != 0 {
            Some(self.index(command.frame.max(1))?)
        } else {
            None
        };
        self.advance(now);
        if let Some(current) = current {
            self.current = current;
        }
        if let Some(frame) = frame {
            self.frames[frame].gap_ms = command.gap.max(0) as u32;
        }
        if command.loops > 0 {
            self.loops = (command.loops > 1).then_some(command.loops - 1);
        }
        if command.state > 0 {
            self.state = command.state;
            self.completed_loops = 0;
        }
        if self.state == 1 {
            self.completed_loops = 0;
            self.deadline = None;
        } else {
            self.deadline =
                now.checked_add(Duration::from_millis(self.current_frame().gap_ms as u64));
            self.advance(now);
        }
        Ok(())
    }

    pub fn delete_frame(&mut self, number: u32, now: Instant) -> Result<bool, String> {
        let index = self.index(number.max(1))?;
        if self.frames.len() == 1 {
            return Ok(true);
        }
        self.frames.remove(index);
        if self.current > index {
            self.current -= 1;
        }
        self.current = self.current.min(self.frames.len() - 1);
        if self.state != 1 {
            self.deadline =
                now.checked_add(Duration::from_millis(self.current_frame().gap_ms as u64));
            self.advance(now);
        }
        Ok(false)
    }

    pub fn advance(&mut self, now: Instant) -> bool {
        let previous = (self.current, self.state, self.deadline);
        if self.state == 1 {
            return false;
        }
        let cycle_ms: u64 = self
            .frames
            .iter()
            .map(|frame| u64::from(frame.gap_ms))
            .sum();
        // An all-gapless animation cannot generate an endless render loop.
        if cycle_ms == 0 {
            self.current = self.frames.len() - 1;
            self.deadline = None;
            return previous != (self.current, self.state, None);
        }
        if let Some(deadline) = self.deadline
            && now >= deadline
            && self.state == 3
        {
            let cycles = now.duration_since(deadline).as_millis() / u128::from(cycle_ms);
            let cycles = cycles.min(u32::MAX as u128) as u32;
            let cycles = self.loops.map_or(cycles, |limit| {
                cycles.min(limit.saturating_sub(self.completed_loops.saturating_add(1)))
            });
            if cycles > 0 {
                self.completed_loops = self.completed_loops.saturating_add(cycles);
                self.deadline =
                    deadline.checked_add(Duration::from_millis(u64::from(cycles) * cycle_ms));
            }
        }
        for _ in 0..=self.frames.len() {
            let Some(deadline) = self.deadline else {
                break;
            };
            if deadline > now {
                break;
            }
            if self.current + 1 == self.frames.len() {
                if self.state == 2 {
                    self.deadline = None;
                    break;
                }
                self.completed_loops = self.completed_loops.saturating_add(1);
                if self
                    .loops
                    .is_some_and(|limit| self.completed_loops >= limit)
                {
                    self.state = 1;
                    self.deadline = None;
                    break;
                }
                self.current = 0;
            } else {
                self.current += 1;
            }
            self.deadline =
                deadline.checked_add(Duration::from_millis(self.current_frame().gap_ms as u64));
        }
        previous != (self.current, self.state, self.deadline)
    }
}

fn check_rect(width: u32, height: u32, x: u32, y: u32, w: u32, h: u32) -> Result<(), String> {
    if w == 0
        || h == 0
        || x.checked_add(w).is_none_or(|end| end > width)
        || y.checked_add(h).is_none_or(|end| end > height)
    {
        Err("EINVAL:animation rectangle out of bounds".into())
    } else {
        Ok(())
    }
}

fn composite(
    destination: &mut [u8],
    destination_width: u32,
    source: &[u8],
    source_width: u32,
    source_origin: (u32, u32),
    destination_origin: (u32, u32),
    size: (u32, u32),
    replace: bool,
) {
    for row in 0..size.1 {
        let source_start = ((source_origin.1 + row) as usize * source_width as usize
            + source_origin.0 as usize)
            * 4;
        let destination_start = ((destination_origin.1 + row) as usize
            * destination_width as usize
            + destination_origin.0 as usize)
            * 4;
        let source = &source[source_start..source_start + size.0 as usize * 4];
        let destination =
            &mut destination[destination_start..destination_start + size.0 as usize * 4];
        if replace {
            destination.copy_from_slice(source);
            continue;
        }
        for (source, destination) in source
            .as_chunks::<4>()
            .0
            .iter()
            .zip(destination.as_chunks_mut::<4>().0)
        {
            let alpha = u32::from(source[3]);
            if alpha == 0 {
                continue;
            }
            if alpha == 255 {
                destination.copy_from_slice(source);
                continue;
            }
            let destination_alpha = u32::from(destination[3]);
            let combined = alpha * 255 + destination_alpha * (255 - alpha);
            for channel in 0..3 {
                destination[channel] = ((u32::from(source[channel]) * alpha * 255
                    + u32::from(destination[channel]) * destination_alpha * (255 - alpha)
                    + combined / 2)
                    / combined) as u8;
            }
            destination[3] = ((combined + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn animation(now: Instant) -> GraphicsAnimation {
        let mut animation = GraphicsAnimation::new(
            Arc::new(GraphicsImage::from_rgba(1, 1, vec![255, 0, 0, 255])),
            1,
        );
        animation
            .control(
                GraphicsAnimationControl {
                    frame: 1,
                    gap: 10,
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        animation
            .update_frame(
                &GraphicsImage::from_rgba(1, 1, vec![0, 255, 0, 255]),
                GraphicsFrameUpdate {
                    gap: 20,
                    ..Default::default()
                },
                2,
                now,
            )
            .unwrap();
        animation
    }

    #[test]
    fn kitty_animation_uses_deadlines_and_stable_texture_generations() {
        let now = Instant::now();
        let mut animation = animation(now);
        animation
            .control(
                GraphicsAnimationControl {
                    state: 3,
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        assert_eq!(animation.current_frame().generation, 1);
        assert!(!animation.advance(now + Duration::from_millis(9)));
        assert!(animation.advance(now + Duration::from_millis(10)));
        assert_eq!(animation.current_frame().generation, 2);
        assert_eq!(
            animation.next_deadline(),
            Some(now + Duration::from_millis(30))
        );
        animation.advance(now + Duration::from_millis(30));
        assert_eq!(animation.current_frame().generation, 1);
        animation.advance(now + Duration::from_secs(86_400));
        assert!(animation.next_deadline().unwrap() > now + Duration::from_secs(86_400));
    }

    #[test]
    fn kitty_animation_obeys_finite_loops_stop_and_restart() {
        let now = Instant::now();
        let mut animation = animation(now);
        animation
            .control(
                GraphicsAnimationControl {
                    state: 3,
                    loops: 2,
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        animation.advance(now + Duration::from_millis(30));
        assert_eq!(animation.current, 1);
        assert!(animation.next_deadline().is_none());
        animation
            .control(
                GraphicsAnimationControl {
                    state: 3,
                    current: 1,
                    ..Default::default()
                },
                now + Duration::from_millis(40),
            )
            .unwrap();
        animation.advance(now + Duration::from_millis(50));
        assert_eq!(animation.current, 1);
        assert!(animation.next_deadline().is_some());
        animation
            .control(
                GraphicsAnimationControl {
                    state: 1,
                    ..Default::default()
                },
                now + Duration::from_millis(55),
            )
            .unwrap();
        assert!(animation.next_deadline().is_none());
    }

    #[test]
    fn kitty_animation_loading_waits_for_new_frames_without_polling() {
        let now = Instant::now();
        let mut animation = animation(now);
        animation
            .control(
                GraphicsAnimationControl {
                    state: 2,
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        animation.advance(now + Duration::from_millis(30));
        assert_eq!(animation.current, 1);
        assert!(animation.next_deadline().is_none());
        animation
            .update_frame(
                &GraphicsImage::from_rgba(1, 1, vec![0, 0, 255, 255]),
                GraphicsFrameUpdate::default(),
                3,
                now + Duration::from_millis(40),
            )
            .unwrap();
        assert_eq!(animation.current_frame().generation, 3);
        assert_eq!(
            animation.next_deadline(),
            Some(now + Duration::from_millis(80))
        );
    }

    #[test]
    fn kitty_all_gapless_frames_do_not_schedule_a_busy_loop() {
        let now = Instant::now();
        let mut animation =
            GraphicsAnimation::new(Arc::new(GraphicsImage::from_rgba(1, 1, vec![255; 4])), 1);
        animation
            .update_frame(
                &GraphicsImage::from_rgba(1, 1, vec![0; 4]),
                GraphicsFrameUpdate {
                    gap: -1,
                    ..Default::default()
                },
                2,
                now,
            )
            .unwrap();
        animation
            .control(
                GraphicsAnimationControl {
                    state: 3,
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        assert!(animation.next_deadline().is_none());
    }
}

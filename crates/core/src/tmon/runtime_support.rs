#[allow(clippy::too_many_arguments)]
fn process_output(
    engine: &Arc<Mutex<Engine>>,
    events: &Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: &Arc<AtomicBool>,
    wakeup_enabled: &Arc<AtomicBool>,
    wakeup_notifier: Option<&WakeupNotifier>,
    sync_batch_active: &Arc<AtomicBool>,
    sync_generation: &Arc<AtomicU64>,
    protocol_reply_sink: &ProtocolReplySink,
    bytes: &[u8],
    queue_wakeup: bool,
) -> Vec<u8> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let (output, watchdog) = {
        let mut engine = engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let output = engine.feed_detailed(bytes);
        let watchdog = if queue_wakeup && output.synchronized_update_active {
            let was_active = sync_batch_active.swap(true, Ordering::AcqRel);
            if !was_active || output.synchronized_update_refreshed {
                let generation = sync_generation.fetch_add(1, Ordering::AcqRel) + 1;
                Some((
                    generation,
                    output
                        .synchronized_update_deadline
                        .unwrap_or_else(|| Instant::now() + SYNC_WATCHDOG_DELAY),
                ))
            } else {
                None
            }
        } else if queue_wakeup {
            sync_batch_active.store(false, Ordering::Release);
            sync_generation.fetch_add(1, Ordering::AcqRel);
            None
        } else {
            None
        };
        (output, watchdog)
    };

    if let Some((generation, deadline)) = watchdog {
        schedule_sync_watchdog(SyncWatchdogTask {
            deadline,
            engine: engine.clone(),
            events: events.clone(),
            wakeup_queued: wakeup_queued.clone(),
            wakeup_enabled: wakeup_enabled.clone(),
            wakeup_notifier: wakeup_notifier.cloned(),
            sync_batch_active: sync_batch_active.clone(),
            sync_generation: sync_generation.clone(),
            protocol_reply_sink: protocol_reply_sink.clone(),
            generation,
        });
    }
    let should_wake = queue_wakeup
        && (!output.synchronized_update_active
            || output.synchronized_update_committed
            || output.unsynchronized_activity);
    queue_events(
        events,
        wakeup_queued,
        wakeup_enabled,
        wakeup_notifier,
        output.events,
        should_wake,
        should_wake,
    );
    output.replies
}

fn schedule_sync_watchdog(task: SyncWatchdogTask) {
    if let Some(scheduler) = sync_watchdog_scheduler() {
        scheduler.schedule(task);
    } else {
        // Thread creation failure is rare, but synchronized updates still
        // need a bounded commit path when the shared worker is unavailable.
        execute_sync_watchdog_task(task);
    }
}

struct SyncWatchdogTask {
    deadline: Instant,
    engine: Arc<Mutex<Engine>>,
    events: Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: Arc<AtomicBool>,
    wakeup_enabled: Arc<AtomicBool>,
    wakeup_notifier: Option<WakeupNotifier>,
    sync_batch_active: Arc<AtomicBool>,
    sync_generation: Arc<AtomicU64>,
    protocol_reply_sink: ProtocolReplySink,
    generation: u64,
}

struct SyncWatchdogScheduler {
    tasks: Mutex<Vec<SyncWatchdogTask>>,
    changed: Condvar,
}

impl SyncWatchdogScheduler {
    fn new() -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
            changed: Condvar::new(),
        }
    }

    fn schedule(&self, task: SyncWatchdogTask) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = tasks
            .iter()
            .position(|queued| Arc::ptr_eq(&queued.engine, &task.engine))
        {
            // A synchronized-update refresh supersedes both the old deadline
            // and generation. Replacing in place keeps at most one pending
            // Arc-heavy task per terminal without blocking on its engine.
            tasks[index] = task;
        } else {
            tasks.push(task);
        }
        drop(tasks);
        self.changed.notify_one();
    }

    fn next_expired(&self) -> SyncWatchdogTask {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            while tasks.is_empty() {
                tasks = self
                    .changed
                    .wait(tasks)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }

            let now = Instant::now();
            let (next_index, next_deadline) = tasks
                .iter()
                .enumerate()
                .min_by_key(|(_, task)| task.deadline)
                .map(|(index, task)| (index, task.deadline))
                .expect("nonempty watchdog queue has an earliest task");
            let wait = next_deadline.saturating_duration_since(now);
            if wait.is_zero() {
                return tasks.swap_remove(next_index);
            }

            let (updated_tasks, _) = self
                .changed
                .wait_timeout(tasks, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            tasks = updated_tasks;
        }
    }
}

fn sync_watchdog_scheduler() -> Option<&'static SyncWatchdogScheduler> {
    static WATCHDOG: OnceLock<Option<Arc<SyncWatchdogScheduler>>> = OnceLock::new();
    WATCHDOG
        .get_or_init(|| {
            let scheduler = Arc::new(SyncWatchdogScheduler::new());
            let worker_scheduler = scheduler.clone();
            std::thread::Builder::new()
                .name("tmon-sync-watchdog".to_string())
                .spawn(move || sync_watchdog_worker(worker_scheduler))
                .ok()
                .map(|_| scheduler)
        })
        .as_deref()
}

fn sync_watchdog_worker(scheduler: Arc<SyncWatchdogScheduler>) {
    loop {
        // `next_expired` releases the scheduler mutex before returning, so a
        // slow parser commit cannot block producer-side deadline refreshes.
        execute_sync_watchdog_task(scheduler.next_expired());
    }
}

fn execute_sync_watchdog_task(task: SyncWatchdogTask) {
    let output = {
        // Atomics are checked while holding the same engine mutex used by the
        // PTY parser. `process_output` publishes generation changes before it
        // releases this lock, so an expired task cannot commit a newer batch.
        let mut engine = task
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if task.sync_generation.load(Ordering::Acquire) != task.generation
            || !task.sync_batch_active.load(Ordering::Acquire)
        {
            return;
        }
        let output = engine.stop_synchronized_update();
        task.sync_batch_active.store(false, Ordering::Release);
        task.sync_generation.fetch_add(1, Ordering::AcqRel);
        output
    };

    if !output.replies.is_empty() {
        (task.protocol_reply_sink)(output.replies);
    }
    queue_events(
        &task.events,
        &task.wakeup_queued,
        &task.wakeup_enabled,
        task.wakeup_notifier.as_ref(),
        output.events,
        true,
        true,
    );
}

fn buffer_protocol_reply(replies: &Arc<Mutex<VecDeque<u8>>>, reply: Vec<u8>) {
    let mut replies = replies
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    buffer_protocol_reply_queue(&mut replies, reply);
}

fn buffer_protocol_reply_queue(replies: &mut VecDeque<u8>, reply: Vec<u8>) {
    let overflow = replies
        .len()
        .saturating_add(reply.len())
        .saturating_sub(MAX_BUFFERED_PROTOCOL_REPLY_BYTES);
    let drain = overflow.min(replies.len());
    replies.drain(..drain);
    if reply.len() >= MAX_BUFFERED_PROTOCOL_REPLY_BYTES {
        replies.clear();
        replies.extend(
            reply[reply.len() - MAX_BUFFERED_PROTOCOL_REPLY_BYTES..]
                .iter()
                .copied(),
        );
    } else {
        replies.extend(reply);
    }
}

fn queue_events(
    events: &Arc<Mutex<VecDeque<Event>>>,
    wakeup_queued: &AtomicBool,
    wakeup_enabled: &Arc<AtomicBool>,
    wakeup_notifier: Option<&WakeupNotifier>,
    incoming: impl IntoIterator<Item = Event>,
    queue_wakeup: bool,
    notify: bool,
) {
    let mut incoming = incoming.into_iter().peekable();
    if incoming.peek().is_none() && queue_wakeup && wakeup_queued.load(Ordering::Acquire) {
        return;
    }

    let (changed, queued_non_wakeup) = {
        let mut queue = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut changed = false;
        let mut queued_non_wakeup = false;
        for event in incoming {
            let is_wakeup = matches!(event, Event::Wakeup);
            if push_bounded_event(&mut queue, event) {
                changed = true;
                if is_wakeup {
                    wakeup_queued.store(true, Ordering::Release);
                } else {
                    queued_non_wakeup = true;
                }
            }
        }
        if queue_wakeup && !wakeup_queued.swap(true, Ordering::AcqRel) {
            changed |= push_bounded_event(&mut queue, Event::Wakeup);
        }
        (changed, queued_non_wakeup)
    };
    if changed
        && notify
        // Hiding a terminal suppresses repaint wakeups, not lifecycle or
        // protocol events the host must drain to make forward progress.
        && (queued_non_wakeup || wakeup_enabled.load(Ordering::Acquire))
        && let Some(notifier) = wakeup_notifier
    {
        notifier.notify();
    }
}

fn push_bounded_event(queue: &mut VecDeque<Event>, event: Event) -> bool {
    let singleton = matches!(event, Event::Wakeup | Event::Exit);
    if singleton
        && queue.iter().any(|queued| {
            matches!(
                (&event, queued),
                (Event::Wakeup, Event::Wakeup) | (Event::Exit, Event::Exit)
            )
        })
    {
        return false;
    }

    if event_is_coalescable_state(&event) {
        // State updates can replace older values inside one trailing state-only
        // segment, but never across a lifecycle/protocol event. The desktop
        // consumes command titles before ShellCommandFinished, so crossing that
        // boundary would lose the command identity for plugins.
        let segment_start = queue
            .iter()
            .rposition(|queued| !event_is_coalescable_state(queued))
            .map_or(0, |index| index + 1);
        let mut index = segment_start;
        while index < queue.len() {
            if event_supersedes(&event, &queue[index]) {
                queue.remove(index);
            } else {
                index += 1;
            }
        }
    }

    // Keep enough room for the remainder of a normal OSC 133 lifecycle. A
    // stalled renderer may fill the queue with bells or replaceable state, but
    // it must not leave the desktop with just half of B/C/D command tracking.
    let priority = event_priority(&event);
    let reserved_followups = lifecycle_followup_reserve(&event);
    let target_len = MAX_QUEUED_EVENTS.saturating_sub(1 + reserved_followups);
    while queue.len() > target_len && remove_lower_priority_event(queue, priority) {}

    if queue.len() == MAX_QUEUED_EVENTS {
        if make_event_room(queue, priority) {
            // Room was made without splitting an older command lifecycle.
        } else if priority < EventPriority::Protocol {
            return false;
        } else if let Some(index) = queue
            .iter()
            .position(|queued| matches!(queued, Event::ClipboardStore(_)))
            .or_else(|| {
                queue
                    .iter()
                    .position(|queued| matches!(queued, Event::ClipboardLoad(_)))
            })
        {
            queue.remove(index);
        } else {
            // Wakeup and Exit are deduplicated. A full queue with neither an
            // older lifecycle nor a replaceable clipboard event cannot occur
            // under the normal producer, so retain it if invariants change.
            return false;
        }
    }
    queue.push_back(event);
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum EventPriority {
    Noise,
    State,
    Lifecycle,
    Protocol,
}

fn event_priority(event: &Event) -> EventPriority {
    match event {
        Event::Bell => EventPriority::Noise,
        Event::Title(_) | Event::ResetTitle | Event::Progress(_) | Event::WorkingDirectory(_) => {
            EventPriority::State
        }
        Event::ShellPromptStart
        | Event::ShellCommandStart
        | Event::ShellCommandExecuting
        | Event::ShellCommandFinished(_) => EventPriority::Lifecycle,
        Event::Wakeup
        | Event::Exit
        | Event::ClipboardLoad(_)
        | Event::ClipboardStore(_)
        | Event::KittyClipboard(_)
        | Event::KittyClipboardMode(_)
        | Event::KittyClipboardReset => EventPriority::Protocol,
    }
}

fn lifecycle_followup_reserve(event: &Event) -> usize {
    match event {
        Event::ShellPromptStart => 3,
        Event::ShellCommandStart => 2,
        Event::ShellCommandExecuting => 1,
        _ => 0,
    }
}

fn remove_lower_priority_event(
    queue: &mut VecDeque<Event>,
    incoming_priority: EventPriority,
) -> bool {
    let Some(lowest) = queue
        .iter()
        .map(event_priority)
        .filter(|priority| *priority < incoming_priority)
        .min()
    else {
        return false;
    };

    if lowest == EventPriority::Lifecycle {
        return remove_oldest_lifecycle_cycle(queue);
    }
    let Some(index) = queue
        .iter()
        .position(|event| event_priority(event) == lowest)
    else {
        return false;
    };
    queue.remove(index);
    true
}

fn make_event_room(queue: &mut VecDeque<Event>, incoming_priority: EventPriority) -> bool {
    if remove_lower_priority_event(queue, incoming_priority) {
        return true;
    }
    incoming_priority == EventPriority::Lifecycle && remove_oldest_lifecycle_cycle(queue)
}

fn remove_oldest_lifecycle_cycle(queue: &mut VecDeque<Event>) -> bool {
    let first = queue
        .iter()
        .position(|event| event_priority(event) == EventPriority::Lifecycle);
    let Some(first) = first else {
        return false;
    };
    let completed_end = queue
        .iter()
        .enumerate()
        .skip(first)
        .find_map(|(index, event)| {
            matches!(event, Event::ShellCommandFinished(_)).then_some(index)
        });
    let end = completed_end.unwrap_or(queue.len().saturating_sub(1));
    let indices = queue
        .iter()
        .enumerate()
        .take(end + 1)
        .filter_map(|(index, event)| {
            (event_priority(event) == EventPriority::Lifecycle).then_some(index)
        })
        .collect::<Vec<_>>();
    for index in indices.into_iter().rev() {
        queue.remove(index);
    }
    true
}

fn event_is_coalescable_state(event: &Event) -> bool {
    matches!(
        event,
        Event::Title(_) | Event::ResetTitle | Event::Progress(_) | Event::WorkingDirectory(_)
    )
}

fn event_supersedes(incoming: &Event, queued: &Event) -> bool {
    matches!(
        (incoming, queued),
        (
            Event::Title(_) | Event::ResetTitle,
            Event::Title(_) | Event::ResetTitle
        ) | (Event::Progress(_), Event::Progress(_))
            | (Event::WorkingDirectory(_), Event::WorkingDirectory(_))
    )
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "windows"
))]
fn resolve_spawn_config(config: Config) -> Result<pty::SpawnConfig, Error> {
    let (program, args) = match config.launch {
        Some(Launch::Program { program, args }) => {
            if program.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "terminal program cannot be empty",
                )
                .into());
            }
            (program, args)
        }
        Some(Launch::ShellCommand(command)) if !command.trim().is_empty() => {
            shell_command_launch(command)
        }
        _ => {
            let program = config
                .shell
                .as_deref()
                .map(str::trim)
                .filter(|shell| !shell.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    std::env::var("SHELL")
                        .ok()
                        .filter(|shell| !shell.trim().is_empty())
                })
                .unwrap_or_else(default_shell);
            let args = login_shell_args(&program);
            (program, args)
        }
    };

    Ok(pty::SpawnConfig {
        program,
        args,
        working_directory: config.working_directory,
        environment: config.environment,
    })
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn shell_command_launch(command: String) -> (String, Vec<String>) {
    ("/bin/sh".to_string(), vec!["-c".to_string(), command])
}

#[cfg(target_os = "windows")]
fn shell_command_launch(command: String) -> (String, Vec<String>) {
    (
        windows_command_processor(),
        vec![
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            command,
        ],
    )
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn default_shell() -> String {
    #[cfg(target_os = "macos")]
    {
        "/bin/zsh".to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "/bin/bash".to_string()
    }
}

#[cfg(target_os = "windows")]
fn default_shell() -> String {
    windows_command_processor()
}

#[cfg(target_os = "windows")]
fn windows_command_processor() -> String {
    std::env::var("COMSPEC")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "cmd.exe".to_string())
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn login_shell_args(program: &str) -> Vec<String> {
    let shell = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str());
    if !matches!(shell, Some("bash" | "zsh" | "fish")) {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        vec!["-i".to_string(), "-l".to_string()]
    }
    #[cfg(not(target_os = "macos"))]
    {
        vec!["-i".to_string()]
    }
}

#[cfg(target_os = "windows")]
fn login_shell_args(_program: &str) -> Vec<String> {
    Vec::new()
}

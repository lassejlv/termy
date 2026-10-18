//! macOS updates go through Sparkle. Release builds embed `Sparkle.framework`
//! in `Termy.app/Contents/Frameworks` (see `scripts/build-dmg.sh`), and it is
//! loaded at runtime so development builds without it still launch. Sparkle
//! brings its own update UI and check schedule; the feed URL and EdDSA public
//! key live in the bundle's Info.plist.

use gpui_kit::{App, Global};
use objc2::{
    msg_send,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_foundation::{NSBundle, NSString};

/// The app-wide `SPUStandardUpdaterController`, or `None` when this build
/// does not embed Sparkle.
struct GlobalSparkle(Option<Retained<AnyObject>>);
impl Global for GlobalSparkle {}

fn controller(cx: &mut App) -> Option<Retained<AnyObject>> {
    if let Some(global) = cx.try_global::<GlobalSparkle>() {
        return global.0.clone();
    }
    let controller = start_controller()
        .inspect_err(|error| log::info!("Sparkle updater unavailable: {error}"))
        .ok();
    cx.set_global(GlobalSparkle(controller.clone()));
    controller
}

/// Starts Sparkle on first use and applies the `auto_update` setting to its
/// scheduled checks. Returns `false` when this build has no Sparkle.
pub fn sync_automatic_checks(enabled: bool, cx: &mut App) -> bool {
    let Some(controller) = controller(cx) else {
        return false;
    };
    // SAFETY: `updater` and `automaticallyChecksForUpdates` are properties of
    // SPUStandardUpdaterController and SPUUpdater, called on the main thread.
    unsafe {
        let updater: Retained<AnyObject> = msg_send![&*controller, updater];
        let current: bool = msg_send![&*updater, automaticallyChecksForUpdates];
        if current != enabled {
            let _: () = msg_send![&*updater, setAutomaticallyChecksForUpdates: enabled];
        }
    }
    true
}

/// Runs a user-initiated check with Sparkle's own progress and result UI.
/// Returns `false` when this build has no Sparkle.
pub fn check_for_updates(cx: &mut App) -> bool {
    let Some(controller) = controller(cx) else {
        return false;
    };
    // SAFETY: -[SPUStandardUpdaterController checkForUpdates:] takes an
    // optional sender and is called on the main thread.
    unsafe {
        let _: () = msg_send![&*controller, checkForUpdates: None::<&AnyObject>];
    }
    true
}

fn start_controller() -> Result<Retained<AnyObject>, String> {
    let frameworks = NSBundle::mainBundle()
        .privateFrameworksPath()
        .ok_or("the app has no Frameworks directory")?;
    let path = format!("{frameworks}/Sparkle.framework");
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(&path))
        .ok_or_else(|| format!("{path} not found"))?;
    // SAFETY: loading runs only Sparkle's own initializers. Release builds sign
    // the framework with the app's team, as library validation requires.
    if !unsafe { bundle.load() } {
        return Err(format!("failed to load {path}"));
    }
    let class = AnyClass::get(c"SPUStandardUpdaterController")
        .ok_or("SPUStandardUpdaterController is missing from Sparkle")?;
    // SAFETY: matches -[SPUStandardUpdaterController
    // initWithStartingUpdater:updaterDelegate:userDriverDelegate:], whose
    // delegates are optional; called on the main thread.
    let controller: Option<Retained<AnyObject>> = unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![
            allocated,
            initWithStartingUpdater: true,
            updaterDelegate: None::<&AnyObject>,
            userDriverDelegate: None::<&AnyObject>
        ]
    };
    controller.ok_or_else(|| "Sparkle failed to start".to_string())
}

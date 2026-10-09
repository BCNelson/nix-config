// Spool KWin script (KWin 6). Loaded by spoold over D-Bus
// (org.kde.kwin.Scripting.loadScript(<this file>, "spool")).
//
// Reports focus changes and the Meta+V shortcut to spoold's
// dev.bcnelson.spool.Kwin interface. Sends only app ids (desktop file name or
// resource class) and KWin's opaque window UUID; never window titles.

const SERVICE = "dev.bcnelson.spool";
const PATH = "/dev/bcnelson/spool";
const IFACE = "dev.bcnelson.spool.Kwin";

function app(w) {
    return w ? (w.desktopFileName || w.resourceClass || "") : "";
}

function wid(w) {
    return w ? w.internalId.toString() : "";
}

function reportActive(w) {
    callDBus(SERVICE, PATH, IFACE, "ActiveWindow", app(w), wid(w));
}

workspace.windowActivated.connect(reportActive);

registerShortcut("spool-show", "Spool: show clipboard history", "Meta+V", function () {
    const c = workspace.cursorPos;
    const w = workspace.activeWindow;
    // `| 0` keeps the coordinates JS integers so callDBus marshals them as
    // D-Bus int32 ("i"), matching Show(iiss).
    callDBus(SERVICE, PATH, IFACE, "Show", c.x | 0, c.y | 0, app(w), wid(w));
});

// Initial state, so spoold knows the active window before the first change.
reportActive(workspace.activeWindow);

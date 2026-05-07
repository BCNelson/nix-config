"use strict";

var AdaptiveWorkspaces = (function () {
    var MOBILE_SCREEN_COUNT = 1;
    var DOCKED_SCREEN_COUNT = 3;
    var DEBOUNCE_MS = 1500;
    var DESKTOP_NAMES = ["Left", "Main", "Right"];

    var debounceTimer = null;
    var lastStableScreenCount = screenInfos().length;
    var lastStableDockedScreens = sortedScreenInfos();
    var pendingUndockPlan = null;
    var connectedOutputSignals = {};

    function log(message) {
        print("[adaptive-workspaces] " + message);
    }

    function toArray(value) {
        var result = [];
        if (!value) {
            return result;
        }

        for (var i = 0; i < value.length; i++) {
            result.push(value[i]);
        }

        return result;
    }

    function numberProperty(object, name) {
        if (!object) {
            return 0;
        }

        var value = object[name];
        if (typeof value === "function") {
            return Number(value.call(object));
        }

        return Number(value || 0);
    }

    function rectInfo(rect) {
        return {
            x: numberProperty(rect, "x"),
            y: numberProperty(rect, "y"),
            width: numberProperty(rect, "width"),
            height: numberProperty(rect, "height")
        };
    }

    function outputKey(output) {
        if (!output) {
            return "";
        }

        var geometry = rectInfo(output.geometry);
        return [
            output.name || "",
            output.manufacturer || "",
            output.model || "",
            output.serialNumber || "",
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height
        ].join("|");
    }

    function screenInfosFromOutputs(outputs) {
        return outputs.map(function (output) {
            var geometry = rectInfo(output.geometry);
            return {
                output: output,
                key: outputKey(output),
                name: output.name || "",
                x: geometry.x,
                y: geometry.y,
                width: geometry.width,
                height: geometry.height
            };
        });
    }

    function screenInfos() {
        return screenInfosFromOutputs(toArray(workspace.screens));
    }

    function sortedScreenInfos() {
        var infos = screenInfos();
        infos.sort(function (a, b) {
            if (a.x !== b.x) {
                return a.x - b.x;
            }
            if (a.y !== b.y) {
                return a.y - b.y;
            }
            return a.name < b.name ? -1 : (a.name > b.name ? 1 : 0);
        });
        return infos;
    }

    function ensureDesktopCount(count) {
        while (toArray(workspace.desktops).length < count) {
            var desktops = toArray(workspace.desktops);
            var position = desktops.length;
            var name = DESKTOP_NAMES[position] || ("Desktop " + (position + 1));
            workspace.createDesktop(position, name);
        }

        return toArray(workspace.desktops).length >= count;
    }

    function desktopIndex(desktop) {
        if (!desktop) {
            return -1;
        }

        if (desktop.x11DesktopNumber) {
            return Number(desktop.x11DesktopNumber) - 1;
        }

        var desktops = toArray(workspace.desktops);
        for (var i = 0; i < desktops.length; i++) {
            if (desktops[i] === desktop || desktops[i].id === desktop.id) {
                return i;
            }
        }

        return -1;
    }

    function desktopForIndex(index) {
        var desktops = toArray(workspace.desktops);
        if (index < 0 || index >= desktops.length) {
            return null;
        }
        return desktops[index];
    }

    function windowDesktops(window) {
        return toArray(window.desktops);
    }

    function isSticky(window) {
        return !!window.onAllDesktops || windowDesktops(window).length === 0;
    }

    function singleDesktopIndex(window) {
        if (isSticky(window)) {
            return -1;
        }

        var desktops = windowDesktops(window);
        if (desktops.length !== 1) {
            return -1;
        }

        return desktopIndex(desktops[0]);
    }

    function activityIds(window) {
        return toArray(window.activities);
    }

    function rootBelongsToCurrentActivity(window) {
        var currentActivity = workspace.currentActivity;
        if (!currentActivity) {
            return true;
        }

        var activities = activityIds(window);
        if (activities.length === 0) {
            return false;
        }

        return activities.indexOf(currentActivity) !== -1;
    }

    function transientBelongsToCurrentActivity(window) {
        var currentActivity = workspace.currentActivity;
        if (!currentActivity) {
            return true;
        }

        var activities = activityIds(window);
        return activities.length === 0 || activities.indexOf(currentActivity) !== -1;
    }

    function isIgnoredWindowType(window) {
        return !!(
            window.desktopWindow ||
            window.dock ||
            window.splash ||
            window.dropdownMenu ||
            window.popupMenu ||
            window.tooltip ||
            window.notification ||
            window.criticalNotification ||
            window.appletPopup ||
            window.onScreenDisplay ||
            window.comboBox ||
            window.dndIcon ||
            window.inputMethod ||
            window.outline
        );
    }

    function isUsableManagedWindow(window) {
        return !!(
            window &&
            window.managed &&
            !window.deleted &&
            !isIgnoredWindowType(window)
        );
    }

    function isRootApplicationWindow(window) {
        return !!(
            isUsableManagedWindow(window) &&
            window.normalWindow &&
            !window.transient &&
            rootBelongsToCurrentActivity(window)
        );
    }

    function internalWindowId(window) {
        if (!window) {
            return "";
        }
        if (window.internalId) {
            return String(window.internalId);
        }
        return String(window.caption || "") + "|" + String(window.resourceClass || "") + "|" + String(window.pid || "");
    }

    function rootForTransient(window, rootById) {
        var seen = {};
        var current = window;

        while (current && current.transientFor) {
            current = current.transientFor;
            var id = internalWindowId(current);
            if (seen[id]) {
                return null;
            }
            seen[id] = true;

            if (rootById[id]) {
                return rootById[id];
            }
        }

        return null;
    }

    function transientGroups() {
        var windows = toArray(workspace.stackingOrder);
        var rootById = {};
        var groups = [];

        for (var i = 0; i < windows.length; i++) {
            var root = windows[i];
            if (!isRootApplicationWindow(root)) {
                continue;
            }

            var rootId = internalWindowId(root);
            var group = {
                root: root,
                windows: [root]
            };
            rootById[rootId] = group;
            groups.push(group);
        }

        for (var j = 0; j < windows.length; j++) {
            var window = windows[j];
            if (!isUsableManagedWindow(window) || !window.transient || !transientBelongsToCurrentActivity(window)) {
                continue;
            }

            var owningGroup = rootForTransient(window, rootById);
            if (!owningGroup) {
                continue;
            }

            owningGroup.windows.push(window);
        }

        return groups;
    }

    function frameGeometry(window) {
        return rectInfo(window.frameGeometry || window.bufferGeometry || {
            x: window.x,
            y: window.y,
            width: window.width,
            height: window.height
        });
    }

    function windowCenter(window) {
        var geometry = frameGeometry(window);
        return {
            x: geometry.x + geometry.width / 2,
            y: geometry.y + geometry.height / 2
        };
    }

    function containsPoint(screen, point) {
        return point.x >= screen.x &&
            point.x < screen.x + screen.width &&
            point.y >= screen.y &&
            point.y < screen.y + screen.height;
    }

    function nearestScreenIndex(screens, point) {
        var bestIndex = -1;
        var bestDistance = null;

        for (var i = 0; i < screens.length; i++) {
            var screenCenterX = screens[i].x + screens[i].width / 2;
            var screenCenterY = screens[i].y + screens[i].height / 2;
            var dx = point.x - screenCenterX;
            var dy = point.y - screenCenterY;
            var distance = dx * dx + dy * dy;

            if (bestDistance === null || distance < bestDistance) {
                bestDistance = distance;
                bestIndex = i;
            }
        }

        return bestIndex;
    }

    function screenIndexForWindow(window, screens) {
        var windowOutput = window.output || window.screen;
        var key = outputKey(windowOutput);
        if (key) {
            for (var i = 0; i < screens.length; i++) {
                if (screens[i].key === key) {
                    return i;
                }
            }
        }

        var center = windowCenter(window);
        for (var j = 0; j < screens.length; j++) {
            if (containsPoint(screens[j], center)) {
                return j;
            }
        }

        return nearestScreenIndex(screens, center);
    }

    function snapshotWindowState(window) {
        return {
            minimized: !!window.minimized,
            fullScreen: !!window.fullScreen
        };
    }

    function restoreWindowState(window, state) {
        if (!window || window.deleted) {
            return;
        }

        try {
            if (state.fullScreen && !window.fullScreen && window.fullScreenable !== false) {
                window.fullScreen = true;
            }
        } catch (error) {
            log("could not restore fullscreen state for " + window.caption + ": " + error);
        }

        try {
            if (state.minimized && !window.minimized && window.minimizable !== false) {
                window.minimized = true;
            }
        } catch (error2) {
            log("could not restore minimized state for " + window.caption + ": " + error2);
        }
    }

    function sendWindowToScreen(window, output) {
        if (!output || !window || window.deleted) {
            return;
        }

        try {
            workspace.sendClientToScreen(window, output);
        } catch (error) {
            log("could not move " + window.caption + " to output: " + error);
        }
    }

    function setWindowDesktop(window, desktop) {
        if (!desktop || !window || window.deleted || isSticky(window)) {
            return;
        }

        try {
            window.desktops = [desktop];
        } catch (error) {
            log("could not move " + window.caption + " to desktop: " + error);
        }
    }

    function applyDockedLayout(reason) {
        var screens = sortedScreenInfos();
        if (screens.length !== DOCKED_SCREEN_COUNT) {
            log("apply-docked-layout ignored for " + screens.length + " screens");
            return false;
        }

        if (!ensureDesktopCount(DOCKED_SCREEN_COUNT)) {
            log("apply-docked-layout could not create enough virtual desktops");
            return false;
        }

        var desktopOne = desktopForIndex(0);
        var groups = transientGroups();
        var transformed = 0;

        for (var i = 0; i < groups.length; i++) {
            var group = groups[i];
            if (isSticky(group.root)) {
                continue;
            }

            var sourceDesktop = singleDesktopIndex(group.root);
            if (sourceDesktop < 0 || sourceDesktop >= DOCKED_SCREEN_COUNT) {
                continue;
            }

            var targetOutput = screens[sourceDesktop].output;
            for (var j = 0; j < group.windows.length; j++) {
                var window = group.windows[j];
                var state = snapshotWindowState(window);
                sendWindowToScreen(window, targetOutput);
                setWindowDesktop(window, desktopOne);
                restoreWindowState(window, state);
                transformed++;
            }
        }

        workspace.currentDesktop = desktopOne;
        log("apply-docked-layout " + reason + " transformed " + transformed + " windows");
        return true;
    }

    function captureMobilePlan(screens) {
        if (!screens || screens.length !== DOCKED_SCREEN_COUNT) {
            return null;
        }

        var groups = transientGroups();
        var entries = [];

        for (var i = 0; i < groups.length; i++) {
            var group = groups[i];
            if (isSticky(group.root) || singleDesktopIndex(group.root) !== 0) {
                continue;
            }

            var screenIndex = screenIndexForWindow(group.root, screens);
            if (screenIndex < 0 || screenIndex >= DOCKED_SCREEN_COUNT) {
                continue;
            }

            entries.push({
                targetDesktopIndex: screenIndex,
                windows: group.windows
            });
        }

        return {
            entries: entries
        };
    }

    function applyMobilePlan(plan, reason) {
        if (!plan) {
            log("apply-mobile-layout " + reason + " had no usable monitor plan");
            return false;
        }

        if (!ensureDesktopCount(DOCKED_SCREEN_COUNT)) {
            log("apply-mobile-layout could not create enough virtual desktops");
            return false;
        }

        var transformed = 0;
        for (var i = 0; i < plan.entries.length; i++) {
            var entry = plan.entries[i];
            var targetDesktop = desktopForIndex(entry.targetDesktopIndex);

            for (var j = 0; j < entry.windows.length; j++) {
                var window = entry.windows[j];
                var state = snapshotWindowState(window);
                setWindowDesktop(window, targetDesktop);
                restoreWindowState(window, state);
                transformed++;
            }
        }

        log("apply-mobile-layout " + reason + " transformed " + transformed + " windows");
        return true;
    }

    function applyMobileLayout(reason) {
        var screens = sortedScreenInfos();
        if (screens.length !== DOCKED_SCREEN_COUNT) {
            log("apply-mobile-layout ignored for " + screens.length + " screens");
            return false;
        }

        return applyMobilePlan(captureMobilePlan(screens), reason);
    }

    function capturePendingUndockPlan() {
        if (lastStableScreenCount !== DOCKED_SCREEN_COUNT || pendingUndockPlan) {
            return;
        }

        var sourceScreens = lastStableDockedScreens.length === DOCKED_SCREEN_COUNT
            ? lastStableDockedScreens
            : sortedScreenInfos();

        pendingUndockPlan = captureMobilePlan(sourceScreens);
    }

    function connectDockedOutputSignals(screens) {
        for (var i = 0; i < screens.length; i++) {
            var screen = screens[i];
            if (!screen.output || connectedOutputSignals[screen.key]) {
                continue;
            }

            if (screen.output.aboutToChange) {
                screen.output.aboutToChange.connect(capturePendingUndockPlan);
            }
            if (screen.output.aboutToTurnOff) {
                screen.output.aboutToTurnOff.connect(capturePendingUndockPlan);
            }

            connectedOutputSignals[screen.key] = true;
        }
    }

    function handleStableTopology() {
        var screens = sortedScreenInfos();
        var stableCount = screens.length;
        var previousCount = lastStableScreenCount;

        if (stableCount === DOCKED_SCREEN_COUNT) {
            lastStableDockedScreens = screens;
            connectDockedOutputSignals(screens);
        }

        if (previousCount === MOBILE_SCREEN_COUNT && stableCount === DOCKED_SCREEN_COUNT) {
            applyDockedLayout("automatic");
        } else if (previousCount === DOCKED_SCREEN_COUNT && stableCount === MOBILE_SCREEN_COUNT) {
            applyMobilePlan(pendingUndockPlan, "automatic");
        }

        if (stableCount !== MOBILE_SCREEN_COUNT && stableCount !== DOCKED_SCREEN_COUNT) {
            pendingUndockPlan = null;
        }

        lastStableScreenCount = stableCount;
        pendingUndockPlan = null;
    }

    function scheduleTopologyCheck() {
        capturePendingUndockPlan();
        debounceTimer.start(DEBOUNCE_MS);
    }

    function makeDebounceTimer() {
        var timer = new QTimer();
        timer.singleShot = true;
        timer.timeout.connect(handleStableTopology);
        return timer;
    }

    function start() {
        debounceTimer = makeDebounceTimer();

        registerShortcut(
            "apply-docked-layout",
            "Apply Docked Layout",
            "",
            function () {
                applyDockedLayout("manual-shortcut");
            }
        );
        registerShortcut(
            "apply-mobile-layout",
            "Apply Mobile Layout",
            "",
            function () {
                applyMobileLayout("manual-shortcut");
            }
        );

        workspace.screensChanged.connect(scheduleTopologyCheck);
        workspace.virtualScreenGeometryChanged.connect(scheduleTopologyCheck);
        if (lastStableScreenCount === DOCKED_SCREEN_COUNT) {
            connectDockedOutputSignals(lastStableDockedScreens);
        }

        log("started with " + lastStableScreenCount + " screens; waiting for next stable transition");
    }

    return {
        start: start,
        applyDockedLayout: applyDockedLayout,
        applyMobileLayout: applyMobileLayout
    };
}());

AdaptiveWorkspaces.start();

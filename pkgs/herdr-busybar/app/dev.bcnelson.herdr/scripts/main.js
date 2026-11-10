// Coding agents on a BUSY Bar: how many are asking for you, working and done.
//
//    ASK    WORK    DONE
//     2      12      5
//          ~~-
//
// Counts come from herdr-busybar on the laptop, over the USB link (see
// ../../../main.go), which reads them from herdr-core: ASK is blocked (and
// error), WORK is working, DONE is done that you have not looked at yet (as
// the herdr app shows it) plus idle agents that finished in the last 30
// minutes (the bridge's -idle-as-done). Older idle agents are not shown.
//
// Layout. The front screen is 72x16: three 24px columns. Four-letter labels
// in the small font (~5px a letter) fit a column, so they share the top row
// (rows 1-5) and the counts get the bold digits (7 rows, 7-13) underneath.
// Row 15 is WORK's bouncing ball, with row 14 between it and the digits.
//
// Motion, kept subtle so it reads from the corner of an eye:
//   - a column with agents gets a soft colour glow, rising from the bottom
//   - ASK's glow breathes (~2s), the one state that wants you
//   - a ball with a fading tail bounces back and forth under WORK's number
//   - a count that changes flashes white for a moment
//   - the LED blinks red when the ASK count goes up
//
// Draws merge by element id, so every element exists from the first frame and
// "hiding" one means drawing it fully transparent. Each tick renders the whole
// scene, compares it per element with what the bar already has, and sends only
// what changed -- usually just the ball. Each draw is an HTTP request to the
// bar itself, so a tick is skipped while one is still out.
//
// Runs on the bar's JerryScript, so: plain ES2017, no browser APIs beyond
// fetch/setTimeout, and nothing allocated per tick that could be avoided.

const ID = "dev.bcnelson.herdr";
const BRIDGE = "http://10.0.4.21:8787/v1/counts";
const DRAW = "http://127.0.0.1/api/display/draw";
const POLL_MS = 1000;

// A frame every 60ms (~16fps). Motion is driven by the clock, not by frame
// count, so a skipped frame (a draw still in flight) only lowers the frame
// rate, never the speed. Measured from the laptop, a one-element draw takes
// ~9ms on the bar.
const TICK_MS = 60;

// Three missed polls (~3s) before calling the laptop gone, so one slow reply
// does not flash the screen.
const AWAY_AFTER = 3;

// A text element anchored top_* has its glyphs start 2px below its y in both
// the small and bold fonts; these are glyph rows, converted when drawing.
const FONT_PAD = 2;
const LABEL_ROW = 1;
const NUMBER_ROW = 7; // digits on rows 7-13; row 14 is air above WORK's ball (15)

const CLEAR = "#00000000";
const FLASH = "#FFFFFFFF";
const FLASH_MS = 350;

// rgb: the column's colour without alpha. dim: a zero's colour, dark enough
// that the eye skips it while the columns stay told apart. x: the column's
// centre; its glow fills the 24px around it.
const COLUMNS = [
  { key: "blocked", label: "ASK", x: 12, rgb: "FF3030", dim: "#5A1414FF" },
  { key: "working", label: "WORK", x: 36, rgb: "FFB000", dim: "#4A3300FF" },
  { key: "done", label: "DONE", x: 60, rgb: "30E060", dim: "#0F4A1EFF" },
];

// Glow strength (alpha byte, hex). ASK steps through BREATH every
// BREATH_STEP_MS: a slow swell and fade, two seconds round.
const GLOW = "30";
const BREATH = ["18", "24", "32", "40", "4C", "40", "32", "24"];
const BREATH_STEP_MS = 250;

// An amber ball bouncing back and forth along WORK's bottom row (row 15, the
// margin under the digits). Eased, but gently: it is fastest mid-sweep and
// slows toward each end without ever stopping -- it still reaches the wall at
// half its average speed (BALL_EASE) and turns at once. Full cosine easing
// (speed 0 at the ends) read as a pause; constant speed read as mechanical.
//
// Its tail is where the ball was a moment ago: pixel k sits at the ball's
// position k * BALL_STEP_MS earlier, fading with age. That makes the bounce
// fold the tail for free -- just after a wall the recent positions crowd
// against it and the tail streams out behind the ball again, with nothing to
// flip. Newer pixels draw over older ones where they overlap. Being spaced in
// time, the tail also shortens as the ball slows into an end and stretches
// through the fast middle: motion blur without asking for it.
//
// It stays inside the column (x 24-47) -- typing dots beside a two-digit bold
// count spilled into DONE's.
const BALL_ROW = 15;
const BALL_LEFT = 24; // the ball travels x 24..47
const BALL_TRAVEL = 23;
const BALL_PERIOD_MS = 2600; // there and back: ~18px a second on average
// Speed swing around the average: 1 - EASE at the walls, 1 + EASE mid-sweep.
// 0 is constant speed; 1 stops at the walls.
const BALL_EASE = 0.5;
// Tail spacing in time: how long the ball takes to move one pixel at its top
// speed, so even mid-sweep the tail pixels touch rather than leaving gaps.
const BALL_STEP_MS = BALL_PERIOD_MS / (2 * BALL_TRAVEL) / (1 + BALL_EASE);
const BALL_TAIL = ["FF", "B8", "80", "58", "38", "22", "12"]; // alpha, head first

let stopped = false;
let misses = 0;
let counts = null; // null while the laptop/herdr-core cannot be reached
let lastBlocked = 0;
let blink = false;
let inFlight = false;
let pollTimer;
let tickTimer; // undefined until set: JerryScript timer ids start at 0

const flashUntil = { blocked: 0, working: 0, done: 0 };
const sent = {}; // id -> JSON of the element as the bar has it

function setCounts(next) {
  const now = Date.now();
  if (counts !== null && next !== null) {
    for (let i = 0; i < COLUMNS.length; i++) {
      const k = COLUMNS[i].key;
      if ((next[k] || 0) !== (counts[k] || 0)) flashUntil[k] = now + FLASH_MS;
    }
  }
  if (next !== null) {
    const blocked = next.blocked || 0;
    if (blocked > lastBlocked) blink = true;
    lastBlocked = blocked;
  }
  counts = next;
}

function rect(id, x, y, w, h, fill, colors, z) {
  return { id: id, type: "rectangle", x: x, y: y, width: w, height: h, fill: fill, fill_colors: colors, border_width: 0, z_index: z };
}

function text(id, x, row, s, font, color) {
  return { id: id, type: "text", x: x, y: row - FONT_PAD, align: "top_mid", text: s, font: font, color: color, z_index: 2 };
}

// The ball's x at time t: out along BALL_TRAVEL and back. Within each leg,
// u runs 0..1 in time and s(u) = u - EASE*sin(2*pi*u)/(2*pi) is how far along
// it is: speed 1 - EASE*cos(2*pi*u), lowest at the walls, highest mid-leg.
function ballX(t) {
  const phase = (((t % BALL_PERIOD_MS) + BALL_PERIOD_MS) % BALL_PERIOD_MS) / BALL_PERIOD_MS;
  const u = phase < 0.5 ? phase * 2 : phase * 2 - 1;
  const s = u - (BALL_EASE * Math.sin(2 * Math.PI * u)) / (2 * Math.PI);
  const along = phase < 0.5 ? s : 1 - s;
  return BALL_LEFT + Math.round(along * BALL_TRAVEL);
}

function scene(now) {
  const els = [];
  for (let i = 0; i < COLUMNS.length; i++) {
    const col = COLUMNS[i];
    const n = counts ? counts[col.key] || 0 : null;
    const color = n ? "#" + col.rgb + "FF" : col.dim;
    const numberColor = n !== null && now < flashUntil[col.key] ? FLASH : color;

    // Background glow, rising from the bottom so the label stays crisp.
    let alpha = null;
    if (n) alpha = col.key === "blocked" ? BREATH[Math.floor(now / BREATH_STEP_MS) % BREATH.length] : GLOW;
    const glow = alpha ? "#" + col.rgb + alpha : CLEAR;
    const fade = "#" + col.rgb + "00";
    els.push(rect("g" + i, col.x - 12, 0, 24, 16, "gradient_v", alpha ? [fade, glow] : [CLEAR, CLEAR], 0));

    els.push(text("l" + i, col.x, LABEL_ROW, col.label, "small", color));
    els.push(text("n" + i, col.x, NUMBER_ROW, n === null ? "-" : String(n), "bold", numberColor));
  }

  const working = counts ? counts.working || 0 : 0;
  for (let k = 0; k < BALL_TAIL.length; k++) {
    const x = working ? ballX(now - k * BALL_STEP_MS) : BALL_LEFT;
    const c = working ? "#FFB000" + BALL_TAIL[k] : CLEAR;
    // z: the head (k=0) on top, so overlapping tail pixels never dim it.
    els.push(rect("b" + k, x, BALL_ROW, 1, 1, "solid", [c], 1 + BALL_TAIL.length - k));
  }
  return els;
}

function frame() {
  if (stopped) return;
  if (inFlight) return; // the bar is still drawing the last frame
  const els = scene(Date.now());
  const changed = [];
  const pending = {};
  for (let i = 0; i < els.length; i++) {
    const json = JSON.stringify(els[i]);
    if (sent[els[i].id] !== json) {
      changed.push(els[i]);
      pending[els[i].id] = json;
    }
  }
  if (!changed.length && !blink) return;

  const body = { application_name: ID, priority: 60, elements: changed.length ? changed : [els[0]] };
  // The bar's LED blinks this colour once: a new agent is waiting on you.
  if (blink) body.led_notification_color = "#FF0000FF";
  blink = false;
  inFlight = true;
  fetch(DRAW, { method: "POST", body: JSON.stringify(body) })
    .then((r) => {
      if (r.ok) for (const id in pending) sent[id] = pending[id];
    })
    .catch(() => {})
    .then(() => {
      inFlight = false;
    });
}

function poll() {
  if (stopped) return;
  fetch(BRIDGE)
    .then((r) => r.json())
    .then((c) => {
      // ok=false: the bridge is up but no herdr-core is running.
      misses = 0;
      setCounts(c.ok ? c : null);
    })
    .catch(() => {
      misses += 1;
      if (misses >= AWAY_AFTER) setCounts(null);
    })
    .then(() => {
      if (!stopped) pollTimer = setTimeout(poll, POLL_MS);
    });
}

function stop() {
  stopped = true;
  if (pollTimer !== undefined) clearTimeout(pollTimer);
  if (tickTimer !== undefined) clearInterval(tickTimer);
  unbind();
}

const unbind = listen("input", (ev) => {
  // Long Back is the firmware's force-quit; a short press quits politely.
  if (ev.key === "back" && ev.action === "short") stop();
});

// Element ids from an older layout would linger -- draws merge by id -- so
// start from an empty canvas.
fetch(DRAW + "?application_name=" + ID, { method: "DELETE" })
  .catch(() => {})
  .then(() => {
    tickTimer = setInterval(frame, TICK_MS);
    poll();
  });

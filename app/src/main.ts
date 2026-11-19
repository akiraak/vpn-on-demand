import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";

// ---------- types (mirror the Rust side) ----------

type Phase = "starting" | "disconnected" | "connecting" | "connected" | "disconnecting";
type Step =
  | { step: "checking" }
  | { step: "removing_other"; region: string }
  | { step: "creating_server"; region: string }
  | { step: "using_existing"; region: string }
  | { step: "waiting_for_server"; seconds: number }
  | { step: "waiting_for_approval"; host_name: string; seconds: number }
  | { step: "switching" }
  | { step: "checking_ip" }
  | { step: "turning_off_exit_node" }
  | { step: "deleting_server"; region: string }
  | { step: "verifying" };
type Server = { id: string; label: string; region: string; plan: string; main_ip: string; created: string };
type IpInfo = { ip: string; country: string; city: string; org: string };
type Connection = {
  server: Server;
  host_name: string;
  hourly_cost: number | null;
  public_ip: IpInfo | null;
  expected_country: string;
};
type UiState = {
  phase: Phase;
  step: Step | null;
  error: string | null;
  notice: string | null;
  connection: Connection | null;
  leftover: Server[];
  auto_disconnect_at: number | null;
  keys_ready: boolean;
  tailscale_error: string | null;
  target_region: string | null;
};
type Region = { id: string; city: string; country: string; continent: string };
type Country = { code: string; continent: string; regions: Region[] };
type Saved = { name: string; masked: string | null };
type Settings = { auto_disconnect_hours: number | null; last_region: string; last_country: string; policy_done: boolean };
type Check = { ok: boolean; message: string };
type SetupReport = { tailscale: Check; vultr: Check; auth_key: Check };

// ---------- helpers ----------

const $ = <T extends HTMLElement = HTMLElement>(sel: string) => document.querySelector(sel) as T;
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
const countryName = new Intl.DisplayNames(["en"], { type: "region" });
const nameOf = (code: string) => {
  try {
    return countryName.of(code) ?? code;
  } catch {
    return code;
  }
};
const icon = (id: string, size = 12) => `<svg width="${size}" height="${size}"><use href="#${id}"/></svg>`;

const ADMIN_MACHINES = "https://login.tailscale.com/admin/machines";

let state: UiState | null = null;
let countries: Country[] = [];
let settings: Settings | null = null;
let firstState = true;
let firstRun = false;
let selCountry = "JP";
let selRegion = "nrt";
let cancelling = false;

function regionInfo(id: string): Region | undefined {
  for (const c of countries) {
    const r = c.regions.find((r) => r.id === id);
    if (r) return r;
  }
  return undefined;
}

function city(id: string): string {
  return regionInfo(id)?.city ?? id;
}

function place(id: string): string {
  const r = regionInfo(id);
  return r ? `${r.city}, ${nameOf(r.country)}` : id;
}

/** 1:05:09 / 5:09 */
function clock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** 2 h 48 min / 12 min */
function hoursMinutes(ms: number): string {
  const total = Math.max(0, Math.round(ms / 60000));
  const h = Math.floor(total / 60);
  const m = total % 60;
  return h > 0 ? `${h} h ${m} min` : `${m} min`;
}

function showTab(tab: string) {
  document.querySelectorAll<HTMLElement>(".tab").forEach((el) => (el.hidden = el.id !== `tab-${tab}`));
  document
    .querySelectorAll<HTMLButtonElement>(".tabs button")
    .forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
  if (tab === "settings") loadKeys();
}

function banner(kind: "error" | "info" | "warn", html: string): string {
  return `<div class="banner ${kind}">${html}</div>`;
}

// ---------- timeline ----------

type Line = { done: string; now: string; help?: string };
type Plan = "connect" | "disconnect";

const CONNECT_STEPS = ["checking", "server", "waiting_for_server", "waiting_for_approval", "switching", "checking_ip"];
const DISCONNECT_STEPS = ["turning_off_exit_node", "deleting_server", "verifying"];

function planOf(s: Step | null, phase: Phase): Plan {
  if (s && ["turning_off_exit_node", "deleting_server", "verifying"].includes(s.step)) return "disconnect";
  if (s) return "connect";
  return phase === "disconnecting" ? "disconnect" : "connect";
}

function indexOf(s: Step | null, plan: Plan): number {
  if (!s) return 0;
  const key = ["removing_other", "creating_server", "using_existing"].includes(s.step) ? "server" : s.step;
  return Math.max(0, (plan === "connect" ? CONNECT_STEPS : DISCONNECT_STEPS).indexOf(key));
}

/** What the timeline remembers while one connect / disconnect runs. */
const progress = {
  plan: null as Plan | null,
  start: 0,
  index: 0,
  doneAt: [] as (number | null)[],
  serverLine: null as Line | null,
  region: "",
};

function trackProgress(st: UiState) {
  const busy = st.phase === "connecting" || st.phase === "disconnecting";
  if (!busy) {
    progress.plan = null;
    cancelling = false;
    return;
  }
  const plan = planOf(st.step, st.phase);
  if (progress.plan !== plan) {
    progress.plan = plan;
    progress.start = Date.now();
    progress.index = 0;
    progress.doneAt = [];
    progress.serverLine = null;
  }
  progress.region = st.target_region ?? st.connection?.server.region ?? st.leftover[0]?.region ?? "";
  const s = st.step;
  if (s && "region" in s && !st.target_region) progress.region = s.region;
  const i = indexOf(s, plan);
  for (let k = progress.index; k < i; k++) progress.doneAt[k] ??= Date.now();
  progress.index = Math.max(progress.index, i);

  if (s?.step === "creating_server") {
    progress.serverLine = { now: `Creating a server in ${esc(city(s.region))}`, done: `Created a server in ${esc(city(s.region))}` };
  } else if (s?.step === "using_existing") {
    progress.serverLine = {
      now: `Using the server already running in ${esc(city(s.region))}`,
      done: `Used the server already running in ${esc(city(s.region))}`,
    };
  } else if (s?.step === "removing_other") {
    progress.serverLine = { now: `Deleting the server in ${esc(city(s.region))}`, done: `Deleted the server in ${esc(city(s.region))}` };
  }
}

function lines(st: UiState): Line[] {
  const where = esc(city(progress.region));
  if (progress.plan === "disconnect") {
    return [
      { now: "Turning off the exit node", done: "Turned off the exit node" },
      { now: `Deleting the server in ${where}`, done: `Deleted the server in ${where}` },
      { now: "Checking that the server is gone", done: "Checked that the server is gone" },
    ];
  }
  const approval: Line = { now: "Waiting for approval as an exit node", done: "Approved as an exit node" };
  if (st.step?.step === "waiting_for_approval") {
    approval.help = `Approve <b>${esc(st.step.host_name)}</b> in the <a href="${ADMIN_MACHINES}">Tailscale admin console</a>
      (… → Edit route settings → Use as exit node), or add <code>autoApprovers</code> to your tailnet policy
      (see Settings) so it happens automatically.`;
  }
  return [
    { now: "Checking Tailscale and Vultr", done: "Checked Tailscale and Vultr" },
    progress.serverLine ?? { now: `Creating a server in ${where}`, done: `Created a server in ${where}` },
    { now: "Server is starting and joining your tailnet", done: "Server joined your tailnet", help: "Usually about 2 minutes" },
    approval,
    { now: "Switching this PC to the server", done: "Switched this PC to the server" },
    { now: "Checking the public IP", done: "Checked the public IP" },
  ];
}

function renderTimeline(st: UiState) {
  const tl = $("#timeline");
  if (!progress.plan) {
    tl.hidden = true;
    return;
  }
  const now = Date.now();
  tl.innerHTML = lines(st)
    .map((l, i) => {
      if (i < progress.index) {
        const at = progress.doneAt[i];
        return `<li class="done"><span class="m">${icon("i-check")}</span><span class="label">${l.done}</span><span class="t">${at ? clock(at - progress.start) : ""}</span></li>`;
      }
      if (i === progress.index) {
        return `<li class="now"><span class="m"></span><span><span class="label">${l.now}…</span>${l.help ? `<small>${l.help}</small>` : ""}</span><span class="t">${clock(now - progress.start)}</span></li>`;
      }
      return `<li><span class="m"></span><span class="label">${l.done}</span><span class="t"></span></li>`;
    })
    .join("");
  tl.hidden = false;
}

// ---------- connect screen ----------

function setNode(id: string, cls: string) {
  $(id).className = `node ${cls}`;
}

function renderRoute(st: UiState) {
  const pulse = $("#n-server .dot");
  pulse.classList.remove("pulse");
  let server = "No server";
  let cap = "";
  let w1 = "idle";
  let w2 = "idle";
  setNode("#n-pc", "live");
  setNode("#n-net", "live");
  setNode("#n-server", "ghost");

  switch (st.phase) {
    case "starting":
      setNode("#n-pc", "");
      setNode("#n-net", "");
      w1 = w2 = "";
      cap = "Checking Tailscale and Vultr…";
      break;
    case "disconnected":
      if (st.leftover.length) {
        setNode("#n-server", "warn");
        server = `${esc(city(st.leftover[0].region))}`;
        w1 = "";
        w2 = "";
        cap = `Not connected, but a server is still running in <b>${esc(place(st.leftover[0].region))}</b>.`;
      } else {
        cap = "Not connected. Traffic goes out from <b>your own connection</b>.";
      }
      break;
    case "connecting":
    case "disconnecting": {
      const region = progress.region;
      setNode("#n-server", "wip");
      pulse.classList.add("pulse");
      server = region ? esc(city(region)) : "Server";
      w1 = "wip";
      w2 = "";
      setNode("#n-net", "");
      if (st.phase === "disconnecting" || progress.plan === "disconnect") {
        cap = st.phase === "connecting" ? "Cancelling…" : `Disconnecting from <b>${esc(place(region))}</b>…`;
      } else {
        cap = `Connecting via <b>${esc(place(region))}</b> · usually 3–5 minutes`;
      }
      break;
    }
    case "connected": {
      const region = st.connection?.server.region ?? "";
      setNode("#n-server", "live");
      server = esc(city(region));
      w1 = w2 = "live";
      cap = `Connected. Traffic goes out from <b>${esc(place(region))}</b>.`;
      break;
    }
  }
  $("#server-name").innerHTML = server;
  $("#w-1").className = `wire ${w1}`;
  $("#w-2").className = `wire ${w2}`;
  $("#route-cap").innerHTML = cap;
}

function renderBanners(st: UiState) {
  const parts: string[] = [];
  if (st.error) parts.push(banner("error", esc(st.error)));
  if (st.notice) parts.push(banner("info", esc(st.notice)));
  if (!st.keys_ready && st.phase !== "starting") {
    parts.push(banner("warn", `Finish the setup checklist in <a href="#" data-goto="settings">Settings</a> first.`));
  }
  if (st.tailscale_error && st.phase !== "connecting") parts.push(banner("warn", esc(st.tailscale_error)));
  if (st.leftover.length && st.phase === "disconnected") {
    const list = st.leftover
      .map((s) => `<b>${esc(place(s.region))}</b> (since ${esc(new Date(s.created).toLocaleString())})`)
      .join(", ");
    const one = st.leftover.length === 1 ? st.leftover[0] : null;
    parts.push(
      banner(
        "warn",
        `A server is still running: ${list}. It costs money until it's deleted.
         <div class="actions"><button class="btn danger" data-action="delete-leftover">Delete it</button>
         ${one ? `<button class="btn outline" data-action="use-leftover" data-region="${esc(one.region)}">Use it</button>` : ""}</div>`,
      ),
    );
  }
  $("#banners").innerHTML = parts.join("");
}

function renderSession(st: UiState) {
  const box = $("#session");
  const c = st.connection;
  if (st.phase !== "connected" || !c) {
    box.hidden = true;
    return;
  }
  box.hidden = false;
  const ip = c.public_ip;
  if (ip) {
    const org = ip.org.replace(/^AS\d+\s+/, "");
    const off = ip.country && c.expected_country && ip.country !== c.expected_country;
    $("#ip").innerHTML =
      `<b>${esc(ip.ip)}</b> <span class="muted">· ${esc([ip.city, ip.country].filter(Boolean).join(", "))}${org ? ` · ${esc(org)}` : ""}</span>` +
      (off ? `<br><span class="error">Not in ${esc(nameOf(c.expected_country))}</span>` : "");
  } else {
    $("#ip").innerHTML = `<span class="muted">Couldn't check</span>`;
  }
  renderTicks(st);
}

/** The numbers that change every second while connected. */
function renderTicks(st: UiState) {
  const c = st.connection;
  if (!c) return;
  const created = Date.parse(c.server.created);
  const elapsed = Number.isNaN(created) ? null : Date.now() - created;
  $("#t-running").textContent = elapsed == null ? "–" : clock(elapsed);
  if (elapsed != null && c.hourly_cost != null) {
    // Vultr bills per started hour, up to the monthly cap.
    const hours = Math.max(1, Math.ceil(elapsed / 3_600_000));
    $("#t-cost").textContent = `$${(hours * c.hourly_cost).toFixed(3)}`;
    $("#t-rate").textContent = `$${c.hourly_cost}/hour`;
  } else {
    $("#t-cost").textContent = "–";
    $("#t-rate").textContent = "";
  }
  const reset = $<HTMLButtonElement>("#t-reset");
  if (st.auto_disconnect_at) {
    $("#t-auto").textContent = hoursMinutes(st.auto_disconnect_at - Date.now());
    reset.hidden = false;
  } else {
    $("#t-auto").textContent = "Off";
    reset.hidden = true;
  }
}

function render() {
  const st = state;
  if (!st) return;
  trackProgress(st);
  renderBanners(st);
  renderRoute(st);
  renderTimeline(st);
  renderSession(st);

  const busy = st.phase === "connecting" || st.phase === "disconnecting" || st.phase === "starting";
  $("#picker").hidden = !(st.phase === "disconnected" || st.phase === "starting");
  const connectBtn = $<HTMLButtonElement>("#connect");
  const disconnectBtn = $<HTMLButtonElement>("#disconnect");
  connectBtn.hidden = !(st.phase === "disconnected" || st.phase === "starting");
  connectBtn.disabled = busy || !st.keys_ready || !regionInfo(selRegion);
  connectBtn.textContent = regionInfo(selRegion) ? `Connect via ${city(selRegion)}` : "Connect";
  disconnectBtn.hidden = connectBtn.hidden === false;
  if (st.phase === "connecting") {
    disconnectBtn.textContent = cancelling || progress.plan === "disconnect" ? "Cancelling…" : "Cancel and delete the server";
    disconnectBtn.disabled = cancelling || progress.plan === "disconnect";
  } else if (st.phase === "disconnecting") {
    disconnectBtn.textContent = "Disconnecting…";
    disconnectBtn.disabled = true;
  } else {
    disconnectBtn.textContent = "Disconnect and delete the server";
    disconnectBtn.disabled = false;
  }
  document.querySelectorAll<HTMLButtonElement>("#countries button, #search").forEach((b) => (b.disabled = busy));
  $<HTMLButtonElement>("#refresh").disabled = busy;

  const hint = $("#cost-hint");
  if (st.phase === "connected") {
    hint.textContent = "Disconnecting deletes the server and stops billing. To switch countries, disconnect first.";
  } else if (st.phase === "disconnected" || st.phase === "starting") {
    hint.textContent = "Connecting creates a small server (about $0.007/hour in most places). Disconnecting deletes it.";
  } else {
    hint.textContent = "You can close this window. The app keeps working in the tray.";
  }

  renderChecklist();
}

// ---------- country list ----------

function renderCountries() {
  const box = $("#countries");
  const q = $<HTMLInputElement>("#search").value.trim().toLowerCase();
  const shown = countries.filter(
    (c) =>
      !q ||
      nameOf(c.code).toLowerCase().includes(q) ||
      c.code.toLowerCase() === q ||
      c.regions.some((r) => r.city.toLowerCase().includes(q)),
  );
  if (!shown.length) {
    box.innerHTML = `<p class="empty">${countries.length ? "No matching country." : "Loading the list of locations…"}</p>`;
    return;
  }
  box.innerHTML = shown
    .map((c) => {
      const sel = c.code === selCountry;
      const summary = c.regions.length > 2 ? `${c.regions.length} cities` : c.regions.map((r) => r.city).join(" · ");
      const row = `<button class="it${sel ? " sel" : ""}" role="option" aria-selected="${sel}" data-country="${esc(c.code)}">
        <span class="cc">${esc(c.code)}</span><span>${esc(nameOf(c.code))}</span><span class="c2">${esc(sel && c.regions.length > 1 ? city(selRegion) : summary)}</span></button>`;
      const cities =
        sel && c.regions.length > 1
          ? `<div class="cities">${c.regions
              .map((r) => `<button class="city${r.id === selRegion ? " sel" : ""}" data-region="${esc(r.id)}">${esc(r.city)}</button>`)
              .join("")}</div>`
          : "";
      return row + cities;
    })
    .join("");
}

function selectCountry(code: string) {
  const c = countries.find((c) => c.code === code);
  if (!c) return;
  selCountry = code;
  if (!c.regions.some((r) => r.id === selRegion)) selRegion = c.regions[0]?.id ?? "";
  renderCountries();
  render();
}

function scrollToSelected() {
  const sel = $("#countries .it.sel");
  if (!sel) return;
  const cities = sel.nextElementSibling?.classList.contains("cities") ? (sel.nextElementSibling as HTMLElement) : null;
  const box = $("#countries");
  const bottom = (cities ?? sel).offsetTop + (cities ?? sel).offsetHeight;
  box.scrollTop = Math.max(0, Math.min(sel.offsetTop - 8, bottom - box.clientHeight + 8));
}

async function loadCountries() {
  try {
    countries = await invoke<Country[]>("list_countries");
  } catch (e) {
    $("#countries").innerHTML = `<p class="empty">Couldn't load the list.</p>`;
    $("#banners").insertAdjacentHTML("afterbegin", banner("error", `Couldn't load the list of locations from Vultr: ${esc(String(e))}`));
    return;
  }
  countries.sort((a, b) => nameOf(a.code).localeCompare(nameOf(b.code), "en"));
  const lastCountry = settings?.last_country ?? "JP";
  selCountry = countries.some((c) => c.code === lastCountry) ? lastCountry : (countries[0]?.code ?? "");
  selRegion = settings?.last_region ?? "";
  selectCountry(selCountry);
  scrollToSelected();
}

async function act(cmd: string, args?: Record<string, unknown>) {
  try {
    await invoke(cmd, args);
  } catch (e) {
    $("#banners").insertAdjacentHTML("afterbegin", banner("error", esc(String(e))));
  }
}

function connect(region: string, country: string) {
  act("connect", { region, country });
}

// ---------- settings: checklist ----------

type ItemId = "tailscale" | "policy" | "tailscale_auth_key" | "vultr_api_key";
const ITEMS: ItemId[] = ["tailscale", "policy", "tailscale_auth_key", "vultr_api_key"];
const savedKeys: Record<string, string | null> = {};
let report: SetupReport | null = null;
let openItem: ItemId | null | undefined; // undefined = not chosen yet

function itemDone(id: ItemId): boolean {
  switch (id) {
    case "tailscale":
      return !!state && state.phase !== "starting" && !state.tailscale_error;
    case "policy":
      return !!settings?.policy_done;
    default:
      return !!savedKeys[id];
  }
}

function testCheck(id: ItemId): Check | null {
  if (!report) return null;
  return id === "tailscale" ? report.tailscale : id === "vultr_api_key" ? report.vultr : id === "tailscale_auth_key" ? report.auth_key : null;
}

function renderChecklist() {
  let done = 0;
  if (openItem === undefined && state && state.phase !== "starting" && Object.keys(savedKeys).length) {
    openItem = ITEMS.find((id) => !itemDone(id)) ?? null;
  }
  for (const id of ITEMS) {
    const el = document.querySelector<HTMLElement>(`.item[data-item="${id}"]`)!;
    const ok = itemDone(id);
    const check = testCheck(id);
    const bad = check ? !check.ok && (id !== "tailscale_auth_key" || ok) : false;
    if (ok) done++;
    el.classList.toggle("done", ok && !bad);
    el.classList.toggle("bad", bad);
    el.classList.toggle("open", openItem === id);
    el.querySelector(".r")!.setAttribute("aria-expanded", String(openItem === id));
    el.querySelector(".m")!.innerHTML = bad ? icon("i-cross", 11) : ok ? icon("i-check", 11) : "";

    const chip = el.querySelector<HTMLElement>(".chipx")!;
    let text = "";
    let cls = "";
    if (check && (ok || id === "tailscale")) {
      text = check.ok ? (id === "vultr_api_key" ? "Works" : id === "tailscale" ? "Checked" : "Looks valid") : "Problem";
      cls = check.ok ? "ok" : "bad";
    } else if (id === "tailscale") {
      text = !state || state.phase === "starting" ? "" : ok ? "Running" : "Not running";
      cls = ok ? "ok" : "bad";
    } else if (id === "policy") {
      text = ok ? "Marked done" : "";
    } else {
      text = ok ? "Saved" : "";
    }
    chip.textContent = text;
    chip.className = `chipx ${cls}`;

    const result = el.querySelector<HTMLElement>(".result");
    if (result) {
      if (check && (ok || id === "tailscale")) {
        result.innerHTML = `<span class="${check.ok ? "ok" : "error"}">${check.ok ? "✓" : "✗"}</span> ${esc(check.message)}`;
      } else if (id === "tailscale" && state?.tailscale_error) {
        result.innerHTML = `<span class="error">✗</span> ${esc(state.tailscale_error)}`;
      } else {
        result.innerHTML = "";
      }
    }
  }
  $("#setup-count").textContent = `${done} of 4 done`;
  $("#policy-done").textContent = settings?.policy_done ? "Mark as not done" : "Mark as done";
  $("#policy-done").classList.toggle("pri", !settings?.policy_done);
  const allOk = done === 4 && !!state?.keys_ready;
  $("#go-connect").hidden = !(firstRun && allOk && report && report.tailscale.ok && report.vultr.ok && report.auth_key.ok);
}

async function loadKeys() {
  try {
    const keys = await invoke<Saved[]>("get_keys");
    for (const k of keys) {
      savedKeys[k.name] = k.masked;
      const row = document.querySelector<HTMLElement>(`.key-row[data-key="${k.name}"]`);
      if (!row) continue;
      row.querySelector(".key-value")!.textContent = k.masked ?? "";
      row.querySelector<HTMLElement>(".saved")!.hidden = !k.masked;
      row.querySelector<HTMLButtonElement>(".remove")!.hidden = !k.masked;
      row.querySelector<HTMLInputElement>("input")!.placeholder = k.masked
        ? "Paste a new key to replace it"
        : k.name === "tailscale_auth_key"
          ? "tskey-auth-…"
          : "Paste the API key";
    }
  } catch (e) {
    keyMsg(`<span class="error">${esc(String(e))}</span>`);
  }
  renderChecklist();
}

function keyMsg(html: string) {
  const el = $("#key-msg");
  el.innerHTML = html;
  el.hidden = !html;
}

function setupKeyForms() {
  document.querySelectorAll<HTMLElement>(".key-row").forEach((row) => {
    const name = row.dataset.key!;
    const label = row.querySelector("input")!.getAttribute("aria-label")!;
    const input = row.querySelector("input")!;
    row.querySelector("form")!.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (!input.value.trim()) return;
      try {
        await invoke("save_key", { name, value: input.value });
        input.value = "";
        report = null;
        keyMsg(`Saved the ${label}.`);
        openItem = undefined; // move on to the next step
      } catch (err) {
        keyMsg(`<span class="error">${esc(String(err))}</span>`);
      }
      loadKeys();
    });
    row.querySelector(".remove")!.addEventListener("click", async () => {
      if (!confirm(`Remove the ${label} from this PC?`)) return;
      try {
        await invoke("delete_key", { name });
        report = null;
        keyMsg(`Removed the ${label}.`);
      } catch (err) {
        keyMsg(`<span class="error">${esc(String(err))}</span>`);
      }
      loadKeys();
    });
  });
}

async function testSetup() {
  const btn = $<HTMLButtonElement>("#test");
  btn.disabled = true;
  btn.textContent = "Testing…";
  try {
    report = await invoke<SetupReport>("test_setup");
    keyMsg("");
    const firstBad = ITEMS.find((id) => testCheck(id)?.ok === false);
    openItem = firstBad ?? openItem;
  } catch (e) {
    keyMsg(`<span class="error">${esc(String(e))}</span>`);
  }
  btn.disabled = false;
  btn.textContent = "Test setup";
  renderChecklist();
}

async function loadSettings() {
  settings = await invoke<Settings>("get_settings");
  $<HTMLSelectElement>("#auto-hours").value = settings.auto_disconnect_hours == null ? "off" : String(settings.auto_disconnect_hours);
}

async function saveSettings() {
  if (settings) await act("save_settings", { settings });
}

// ---------- quit dialog ----------

function showQuitDialog() {
  $("#quit-error").hidden = true;
  $("#quit-dialog").hidden = false;
}

async function quit(del: boolean) {
  const buttons = document.querySelectorAll<HTMLButtonElement>("#quit-dialog button");
  buttons.forEach((b) => (b.disabled = true));
  try {
    await invoke("quit_app", { delete: del });
  } catch (e) {
    const err = $("#quit-error");
    err.textContent = String(e);
    err.hidden = false;
  }
  buttons.forEach((b) => (b.disabled = false));
}

// ---------- wiring ----------

function onState(st: UiState) {
  state = st;
  if (firstState && st.phase !== "starting") {
    firstState = false;
    if (!st.keys_ready) {
      firstRun = true;
      $("#welcome").hidden = false;
      showTab("settings");
    }
  }
  render();
}

window.addEventListener("DOMContentLoaded", async () => {
  document.querySelectorAll<HTMLButtonElement>(".tabs button").forEach((b) => b.addEventListener("click", () => showTab(b.dataset.tab!)));

  // Links open in the browser; in-app actions are delegated.
  document.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    const a = t.closest("a");
    if (a) {
      e.preventDefault();
      if (a.dataset.goto) showTab(a.dataset.goto);
      else if (a.href.startsWith("http")) openUrl(a.href);
      return;
    }
    const country = t.closest<HTMLButtonElement>("button[data-country]");
    if (country) selectCountry(country.dataset.country!);
    const cityBtn = t.closest<HTMLButtonElement>("button[data-region]:not([data-action])");
    if (cityBtn) {
      selRegion = cityBtn.dataset.region!;
      renderCountries();
      render();
    }
    const head = t.closest<HTMLButtonElement>(".item > .r");
    if (head) {
      const id = head.parentElement!.dataset.item as ItemId;
      openItem = openItem === id ? null : id;
      renderChecklist();
    }
    const btn = t.closest<HTMLButtonElement>("button[data-action]");
    if (btn) {
      const action = btn.dataset.action;
      if (action === "delete-leftover") act("disconnect");
      if (action === "use-leftover") {
        const r = regionInfo(btn.dataset.region!);
        if (r) connect(r.id, r.country);
      }
      if (action === "extend") act("extend_auto_disconnect");
    }
    const copy = t.closest<HTMLButtonElement>("button[data-copy]");
    if (copy) {
      navigator.clipboard.writeText($(`#${copy.dataset.copy}`).textContent ?? "");
      copy.textContent = "Copied";
      setTimeout(() => (copy.textContent = "Copy"), 1500);
    }
  });

  $("#search").addEventListener("input", renderCountries);
  $("#connect").addEventListener("click", () => {
    const r = regionInfo(selRegion);
    if (r) connect(r.id, r.country);
  });
  $("#disconnect").addEventListener("click", () => {
    if (state?.phase === "connecting") cancelling = true;
    render();
    act("disconnect");
  });
  $("#refresh").addEventListener("click", () => act("refresh"));
  $("#test").addEventListener("click", testSetup);
  $("#go-connect").addEventListener("click", () => {
    firstRun = false;
    $("#welcome").hidden = true;
    showTab("main");
    render();
  });
  $("#policy-done").addEventListener("click", async () => {
    if (!settings) return;
    settings.policy_done = !settings.policy_done;
    if (settings.policy_done && openItem === "policy") openItem = undefined;
    await saveSettings();
    renderChecklist();
  });
  $("#auto-hours").addEventListener("change", () => {
    if (!settings) return;
    const v = $<HTMLSelectElement>("#auto-hours").value;
    settings.auto_disconnect_hours = v === "off" ? null : Number(v);
    saveSettings();
  });
  $("#quit-delete").addEventListener("click", () => quit(true));
  $("#quit-keep").addEventListener("click", () => quit(false));
  $("#quit-cancel").addEventListener("click", () => ($("#quit-dialog").hidden = true));
  setupKeyForms();

  await listen<UiState>("state", (e) => onState(e.payload));
  await listen("confirm-quit", showQuitDialog);

  $("#version").textContent = `VPN on Demand ${await getVersion()}`;
  await loadSettings();
  await loadKeys();
  onState(await invoke<UiState>("get_state"));
  await loadCountries();
  render();

  // Keep the clocks on screen up to date.
  setInterval(() => {
    if (state?.phase === "connected") renderTicks(state);
    if (progress.plan && state) renderTimeline(state);
  }, 1000);
});

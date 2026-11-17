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
type Settings = { auto_disconnect_hours: number | null; last_region: string; last_country: string };
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

const ADMIN_MACHINES = "https://login.tailscale.com/admin/machines";

let state: UiState | null = null;
let countries: Country[] = [];
let settings: Settings | null = null;
let firstState = true;

function regionInfo(id: string): Region | undefined {
  for (const c of countries) {
    const r = c.regions.find((r) => r.id === id);
    if (r) return r;
  }
  return undefined;
}

function place(id: string): string {
  const r = regionInfo(id);
  return r ? `${r.city}, ${nameOf(r.country)}` : id;
}

function duration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h} h ${String(m).padStart(2, "0")} min`;
  if (m > 0) return `${m} min ${String(s).padStart(2, "0")} s`;
  return `${s} s`;
}

function mmss(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

function showTab(tab: string) {
  document.querySelectorAll<HTMLElement>(".tab").forEach((el) => (el.hidden = el.id !== `tab-${tab}`));
  document
    .querySelectorAll<HTMLButtonElement>(".tabs button")
    .forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
  if (tab === "settings") loadKeys();
}

// ---------- main screen ----------

function stepText(s: Step): string {
  switch (s.step) {
    case "checking":
      return "Checking Tailscale and Vultr…";
    case "removing_other":
      return `Deleting the server in ${esc(place(s.region))}…`;
    case "creating_server":
      return `Creating a server in ${esc(place(s.region))}…`;
    case "using_existing":
      return `Using the server that is already running in ${esc(place(s.region))}…`;
    case "waiting_for_server":
      return `Waiting for the server to start and join your tailnet (${mmss(s.seconds)}). This usually takes about 2 minutes.`;
    case "waiting_for_approval":
      return `Waiting for <b>${esc(s.host_name)}</b> to be approved as an exit node (${mmss(s.seconds)}).
        Approve it in the <a href="${ADMIN_MACHINES}">Tailscale admin console</a> (… → Edit route settings → Use as exit node),
        or add <code>autoApprovers</code> to your tailnet policy (see Settings) so it happens automatically next time.`;
    case "switching":
      return "Switching this PC to the new exit node…";
    case "checking_ip":
      return "Checking your public IP address…";
    case "turning_off_exit_node":
      return "Turning off the exit node…";
    case "deleting_server":
      return `Deleting the server in ${esc(place(s.region))}…`;
    case "verifying":
      return "Checking that the server is gone…";
  }
}

function banner(kind: "error" | "info" | "warn", html: string): string {
  return `<div class="banner ${kind}">${html}</div>`;
}

function renderBanners(st: UiState) {
  const parts: string[] = [];
  if (st.error) parts.push(banner("error", esc(st.error)));
  if (st.notice) parts.push(banner("info", esc(st.notice)));
  if (!st.keys_ready && st.phase !== "starting") {
    parts.push(
      banner("warn", `Add your Vultr API key and Tailscale auth key in <a href="#" data-goto="settings">Settings</a> first.`),
    );
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
        `A server is still running: ${list}. It costs money until it is deleted.
         <div class="actions"><button class="danger" data-action="delete-leftover">Delete it</button>
         ${one ? `<button data-action="use-leftover" data-region="${esc(one.region)}">Use it</button>` : ""}</div>`,
      ),
    );
  }
  $("#banners").innerHTML = parts.join("");
}

function renderDetails(st: UiState) {
  const table = $("#details");
  const c = st.connection;
  if (st.phase !== "connected" || !c) {
    table.hidden = true;
    return;
  }
  const rows: [string, string][] = [];
  rows.push(["Exit location", esc(place(c.server.region))]);
  if (c.public_ip) {
    const ip = c.public_ip;
    const warn = ip.country && c.expected_country && ip.country !== c.expected_country ? ' <span class="error">(not the country you picked)</span>' : "";
    rows.push(["Public IP", `${esc(ip.ip)}<br><span class="hint">${esc([ip.city, nameOf(ip.country)].filter(Boolean).join(", "))} · ${esc(ip.org)}</span>${warn}`]);
  } else {
    rows.push(["Public IP", '<span class="hint">Couldn\'t check</span>']);
  }
  const created = Date.parse(c.server.created);
  if (!Number.isNaN(created)) {
    const elapsed = Date.now() - created;
    rows.push(["Server running for", duration(elapsed)]);
    if (c.hourly_cost != null) {
      // Vultr bills per started hour, up to the monthly cap.
      const hours = Math.max(1, Math.ceil(elapsed / 3_600_000));
      rows.push(["Estimated cost", `$${(hours * c.hourly_cost).toFixed(3)} <span class="hint">($${c.hourly_cost}/h)</span>`]);
    }
  }
  if (st.auto_disconnect_at) {
    rows.push([
      "Auto-disconnect",
      `in ${duration(st.auto_disconnect_at - Date.now())} <button class="link-button" data-action="extend">Reset timer</button>`,
    ]);
  } else {
    rows.push(["Auto-disconnect", '<span class="hint">Off</span>']);
  }
  table.innerHTML = rows.map(([k, v]) => `<tr><th>${k}</th><td>${v}</td></tr>`).join("");
  table.hidden = false;
}

function render() {
  const st = state;
  if (!st) return;
  const dot = $("#status-dot");
  dot.className = `dot ${st.phase}${st.phase === "disconnected" && st.leftover.length ? " leftover" : ""}`;
  const text: Record<Phase, string> = {
    starting: "Checking…",
    disconnected: st.leftover.length ? "Not connected (a server is still running)" : "Not connected",
    connecting: `Connecting to ${st.target_region ? place(st.target_region) : "…"}`,
    connected: st.connection ? `Connected – ${place(st.connection.server.region)}` : "Connected",
    disconnecting: "Disconnecting…",
  };
  $("#status-text").textContent = text[st.phase];

  const step = $("#step");
  step.hidden = !st.step;
  step.innerHTML = st.step ? stepText(st.step) : "";

  renderBanners(st);
  renderDetails(st);

  const busy = st.phase === "connecting" || st.phase === "disconnecting" || st.phase === "starting";
  const connectBtn = $<HTMLButtonElement>("#connect");
  const disconnectBtn = $<HTMLButtonElement>("#disconnect");
  connectBtn.hidden = st.phase === "connected" || st.phase === "connecting" || st.phase === "disconnecting";
  connectBtn.disabled = busy || !st.keys_ready || countries.length === 0;
  disconnectBtn.hidden = !(st.phase === "connected" || st.phase === "connecting" || st.phase === "disconnecting");
  disconnectBtn.textContent = st.phase === "connecting" ? "Cancel" : st.phase === "disconnecting" ? "Disconnecting…" : "Disconnect";
  disconnectBtn.disabled = st.phase === "disconnecting";
  $<HTMLSelectElement>("#country").disabled = busy || st.phase === "connected";
  $<HTMLSelectElement>("#city").disabled = busy || st.phase === "connected" || cityCount() <= 1;
  $<HTMLButtonElement>("#refresh").disabled = busy;

  if (st.phase === "connected" && st.connection) {
    $("#cost-hint").textContent = "To switch countries, disconnect first. Disconnecting deletes the server and stops billing.";
  } else {
    $("#cost-hint").textContent = "Connecting creates a small server (about $0.007/hour in most places). Disconnecting deletes it.";
  }
}

function cityCount(): number {
  return countries.find((c) => c.code === $<HTMLSelectElement>("#country").value)?.regions.length ?? 0;
}

function fillCities(selected?: string) {
  const code = $<HTMLSelectElement>("#country").value;
  const c = countries.find((c) => c.code === code);
  const city = $<HTMLSelectElement>("#city");
  city.innerHTML = (c?.regions ?? []).map((r) => `<option value="${esc(r.id)}">${esc(r.city)}</option>`).join("");
  if (selected && c?.regions.some((r) => r.id === selected)) city.value = selected;
  render();
}

async function loadCountries() {
  try {
    countries = await invoke<Country[]>("list_countries");
  } catch (e) {
    $("#country").innerHTML = "<option>Couldn't load the list</option>";
    $("#banners").insertAdjacentHTML("afterbegin", banner("error", `Couldn't load the list of locations from Vultr: ${esc(String(e))}`));
    return;
  }
  countries.sort((a, b) => nameOf(a.code).localeCompare(nameOf(b.code), "en"));
  const sel = $<HTMLSelectElement>("#country");
  sel.innerHTML = countries
    .map((c) => `<option value="${esc(c.code)}">${esc(nameOf(c.code))}${c.regions.length > 1 ? ` (${c.regions.length} cities)` : ""}</option>`)
    .join("");
  const lastCountry = settings?.last_country ?? "JP";
  sel.value = countries.some((c) => c.code === lastCountry) ? lastCountry : (countries[0]?.code ?? "");
  fillCities(settings?.last_region);
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

// ---------- settings screen ----------

async function loadKeys() {
  try {
    const keys = await invoke<Saved[]>("get_keys");
    for (const k of keys) {
      const row = document.querySelector<HTMLElement>(`.key-row[data-key="${k.name}"]`);
      if (!row) continue;
      row.querySelector(".key-value")!.textContent = k.masked ?? "Not set";
      row.querySelector<HTMLButtonElement>(".remove")!.disabled = !k.masked;
    }
  } catch (e) {
    $("#key-msg").innerHTML = `<span class="error">${esc(String(e))}</span>`;
  }
}

function setupKeyForms() {
  document.querySelectorAll<HTMLElement>(".key-row").forEach((row) => {
    const name = row.dataset.key!;
    const label = row.querySelector(".key-label")!.textContent;
    const input = row.querySelector("input")!;
    row.querySelector("form")!.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (!input.value.trim()) return;
      try {
        await invoke("save_key", { name, value: input.value });
        input.value = "";
        $("#key-msg").textContent = `Saved the ${label}.`;
      } catch (err) {
        $("#key-msg").innerHTML = `<span class="error">${esc(String(err))}</span>`;
      }
      loadKeys();
    });
    row.querySelector(".remove")!.addEventListener("click", async () => {
      if (!confirm(`Remove the ${label} from this PC?`)) return;
      try {
        await invoke("delete_key", { name });
        $("#key-msg").textContent = `Removed the ${label}.`;
      } catch (err) {
        $("#key-msg").innerHTML = `<span class="error">${esc(String(err))}</span>`;
      }
      loadKeys();
    });
  });
}

async function testSetup() {
  const table = $("#test-results");
  const btn = $<HTMLButtonElement>("#test");
  btn.disabled = true;
  table.hidden = false;
  table.innerHTML = "<tr><td>Testing…</td></tr>";
  try {
    const r = await invoke<SetupReport>("test_setup");
    const row = (name: string, c: Check) =>
      `<tr><th>${name}</th><td><span class="${c.ok ? "ok" : "error"}">${c.ok ? "✓" : "✗"}</span> ${esc(c.message)}</td></tr>`;
    table.innerHTML = row("Tailscale", r.tailscale) + row("Vultr API key", r.vultr) + row("Tailscale auth key", r.auth_key);
  } catch (e) {
    table.innerHTML = `<tr><td class="error">${esc(String(e))}</td></tr>`;
  }
  btn.disabled = false;
}

async function loadSettings() {
  settings = await invoke<Settings>("get_settings");
  $<HTMLSelectElement>("#auto-hours").value = settings.auto_disconnect_hours == null ? "off" : String(settings.auto_disconnect_hours);
}

async function saveAutoHours() {
  if (!settings) return;
  const v = $<HTMLSelectElement>("#auto-hours").value;
  settings.auto_disconnect_hours = v === "off" ? null : Number(v);
  await act("save_settings", { settings });
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

  $("#country").addEventListener("change", () => fillCities());
  $("#city").addEventListener("change", render);
  $("#connect").addEventListener("click", () => {
    const region = $<HTMLSelectElement>("#city").value;
    const country = $<HTMLSelectElement>("#country").value;
    if (region) connect(region, country);
  });
  $("#disconnect").addEventListener("click", () => act("disconnect"));
  $("#refresh").addEventListener("click", () => act("refresh"));
  $("#test").addEventListener("click", testSetup);
  $("#auto-hours").addEventListener("change", saveAutoHours);
  $("#quit-delete").addEventListener("click", () => quit(true));
  $("#quit-keep").addEventListener("click", () => quit(false));
  $("#quit-cancel").addEventListener("click", () => ($("#quit-dialog").hidden = true));
  setupKeyForms();

  await listen<UiState>("state", (e) => onState(e.payload));
  await listen("confirm-quit", showQuitDialog);

  $("#version").textContent = `VPN on Demand ${await getVersion()}`;
  await loadSettings();
  onState(await invoke<UiState>("get_state"));
  await loadCountries();
  render();

  // Keep the timers on screen up to date.
  setInterval(() => state?.phase === "connected" && renderDetails(state), 1000);
});

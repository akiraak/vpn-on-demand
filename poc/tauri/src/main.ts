import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Peer = { host_name: string; os: string; online: boolean };
type Status = {
  cli_path: string;
  version: string;
  backend_state: string;
  tailnet: string;
  self_host_name: string;
  self_dns_name: string;
  self_online: boolean;
  exit_node: Peer | null;
  exit_node_options: Peer[];
};
type Region = { id: string; city: string; country: string; continent: string };
type Country = { code: string; continent: string; regions: Region[] };
type Saved = { name: string; masked: string | null };

const SECRET_NAMES: Record<string, string> = {
  vultr_api_key: "Vultr の API キー",
  tailscale_auth_key: "Tailscale の auth key",
};

const $ = <T extends HTMLElement>(sel: string) => document.querySelector(sel) as T;

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

function showTab(tab: string) {
  document.querySelectorAll<HTMLElement>(".tab").forEach((el) => {
    el.hidden = el.id !== `tab-${tab}`;
  });
  document.querySelectorAll<HTMLButtonElement>(".tabs button").forEach((b) => {
    b.classList.toggle("active", b.dataset.tab === tab);
  });
  if (tab === "status") loadStatus();
  if (tab === "regions" && countries.length === 0) loadRegions();
  if (tab === "settings") loadSecrets();
}

// ---- 状態 ----

async function loadStatus() {
  const body = $("#status-body");
  body.textContent = "読み込み中…";
  try {
    const s = await invoke<Status>("tailscale_status");
    const exit = s.exit_node
      ? `${esc(s.exit_node.host_name)}（${s.exit_node.online ? "オンライン" : "オフライン"}）`
      : "使っていない";
    const options = s.exit_node_options.length
      ? s.exit_node_options.map((p) => esc(p.host_name)).join("、")
      : "なし";
    body.innerHTML = `
      <table>
        <tr><th>自分のホスト名</th><td>${esc(s.self_host_name)}</td></tr>
        <tr><th>DNS 名</th><td>${esc(s.self_dns_name)}</td></tr>
        <tr><th>状態</th><td>${esc(s.backend_state)}${s.self_online ? "（オンライン）" : ""}</td></tr>
        <tr><th>tailnet</th><td>${esc(s.tailnet)}</td></tr>
        <tr><th>出口ノード</th><td>${exit}</td></tr>
        <tr><th>選べる出口ノード</th><td>${options}</td></tr>
        <tr><th>Tailscale</th><td>${esc(s.version)}</td></tr>
        <tr><th>CLI</th><td><code>${esc(s.cli_path)}</code></td></tr>
      </table>`;
  } catch (e) {
    body.innerHTML = `<p class="error">${esc(String(e))}</p>`;
  }
}

// ---- 国とリージョン ----

let countries: Country[] = [];
const countryName = new Intl.DisplayNames(["ja"], { type: "region" });

function fillCities() {
  const code = $<HTMLSelectElement>("#country").value;
  const c = countries.find((c) => c.code === code);
  const city = $<HTMLSelectElement>("#city");
  city.innerHTML = (c?.regions ?? [])
    .map((r) => `<option value="${esc(r.id)}">${esc(r.city)}（${esc(r.id)}）</option>`)
    .join("");
  city.disabled = (c?.regions.length ?? 0) <= 1;
}

async function loadRegions() {
  const note = $("#regions-note");
  note.textContent = "読み込み中…";
  try {
    countries = await invoke<Country[]>("vultr_countries");
    countries.sort((a, b) => countryName.of(a.code)!.localeCompare(countryName.of(b.code)!, "ja"));
    const sel = $<HTMLSelectElement>("#country");
    sel.innerHTML = countries
      .map(
        (c) =>
          `<option value="${esc(c.code)}">${esc(countryName.of(c.code) ?? c.code)}${
            c.regions.length > 1 ? `（${c.regions.length} か所）` : ""
          }</option>`,
      )
      .join("");
    sel.value = countries.some((c) => c.code === "JP") ? "JP" : (countries[0]?.code ?? "");
    fillCities();
    const regionCount = countries.reduce((n, c) => n + c.regions.length, 0);
    note.textContent = `${countries.length} か国、${regionCount} リージョン`;
  } catch (e) {
    note.innerHTML = `<span class="error">${esc(String(e))}</span>`;
  }
}

// ---- 設定 ----

async function loadSecrets() {
  const table = $("#secret-table");
  const rows: string[] = [];
  for (const [name, label] of Object.entries(SECRET_NAMES)) {
    try {
      const s = await invoke<Saved>("secret_load", { name });
      rows.push(`<tr><th>${label}</th><td>${s.masked ? `<code>${esc(s.masked)}</code>` : "未保存"}</td></tr>`);
    } catch (e) {
      rows.push(`<tr><th>${label}</th><td class="error">${esc(String(e))}</td></tr>`);
    }
  }
  table.innerHTML = rows.join("");
}

async function saveSecret(e: Event) {
  e.preventDefault();
  const name = $<HTMLSelectElement>("#secret-name").value;
  const input = $<HTMLInputElement>("#secret-value");
  const msg = $("#secret-msg");
  if (!input.value) {
    msg.textContent = "値を入れてください";
    return;
  }
  try {
    await invoke("secret_save", { name, value: input.value });
    input.value = "";
    msg.textContent = `${SECRET_NAMES[name]} を保存しました`;
  } catch (err) {
    msg.innerHTML = `<span class="error">${esc(String(err))}</span>`;
  }
  loadSecrets();
}

async function deleteSecret() {
  const name = $<HTMLSelectElement>("#secret-name").value;
  const msg = $("#secret-msg");
  try {
    await invoke("secret_delete", { name });
    msg.textContent = `${SECRET_NAMES[name]} を消しました`;
  } catch (err) {
    msg.innerHTML = `<span class="error">${esc(String(err))}</span>`;
  }
  loadSecrets();
}

window.addEventListener("DOMContentLoaded", () => {
  document.querySelectorAll<HTMLButtonElement>(".tabs button").forEach((b) => {
    b.addEventListener("click", () => showTab(b.dataset.tab!));
  });
  $("#status-refresh").addEventListener("click", loadStatus);
  $("#regions-refresh").addEventListener("click", loadRegions);
  $("#country").addEventListener("change", fillCities);
  $("#secret-form").addEventListener("submit", saveSecret);
  $("#secret-delete").addEventListener("click", deleteSecret);
  listen<string>("show-tab", (ev) => showTab(ev.payload));
  showTab("status");
});

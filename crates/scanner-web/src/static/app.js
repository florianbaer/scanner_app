// Browser UI for scanner-web. Talks only to the JSON API in routes.rs.
"use strict";

const $ = (id) => document.getElementById(id);
const SETTINGS_KEY = "scanner.settings.v1";

const state = {
  devices: [],
  capabilities: null,
  jobs: [],
  pollTimer: null,
  lastFinishedIds: new Set(),
};

// --- API -------------------------------------------------------------------

async function api(path, options = {}) {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });
  const body = res.status === 204 ? null : await res.json().catch(() => null);
  if (!res.ok) throw new Error(body?.error || `${res.status} ${res.statusText}`);
  return body;
}

// --- devices ---------------------------------------------------------------

async function loadDevices() {
  const select = $("device");
  select.disabled = true;
  $("rescan").disabled = true;
  select.innerHTML = "<option>Searching…</option>";
  $("device-hint").textContent = "Looking for scanners on the network (a few seconds)…";
  try {
    const { devices, cached } = await api("/api/devices");
    state.devices = devices;
    if (devices.length === 0) {
      select.innerHTML = "<option>No scanner found</option>";
      $("device-hint").textContent = "Is the scanner switched on and on the same network?";
      updateStartButton();
      return;
    }
    const saved = loadSettings().device;
    select.innerHTML = "";
    for (const d of devices) {
      const opt = new Option(`${d.label}`, d.name);
      opt.title = d.name;
      select.add(opt);
    }
    if (saved && devices.some((d) => d.name === saved)) select.value = saved;
    select.disabled = false;
    $("device-hint").textContent = cached
      ? "Scanner busy, showing the last known list."
      : `${devices.length} found · ${select.value}`;
    await loadCapabilities();
  } catch (e) {
    select.innerHTML = "<option>Search failed</option>";
    $("device-hint").textContent = e.message;
  } finally {
    $("rescan").disabled = false;
    updateStartButton();
  }
}

async function loadCapabilities() {
  const device = $("device").value;
  if (!device) return;
  $("device-hint").textContent = `Reading settings from ${device}…`;
  try {
    const caps = await api(`/api/capabilities?device=${encodeURIComponent(device)}`);
    state.capabilities = caps;
    applyCapabilities(caps);
    $("device-hint").textContent = device;
  } catch (e) {
    state.capabilities = null;
    $("device-hint").textContent = `Couldn't read scanner settings: ${e.message}`;
  }
  updateStartButton();
}

function applyCapabilities(caps) {
  const saved = loadSettings();
  const res = $("resolution");
  const resolutions = caps.resolutions.length ? caps.resolutions : [150, 300, 600];
  res.innerHTML = "";
  for (const r of resolutions) res.add(new Option(`${r} dpi`, r));
  const wanted = Number(saved.resolution) || 300;
  res.value = resolutions.includes(wanted) ? wanted : nearest(resolutions, wanted);

  for (const input of document.querySelectorAll('input[name="mode"]')) {
    input.disabled = caps.modes.length > 0 && !caps.modes.includes(input.value);
  }
  setDisabled("duplex", !caps.duplex);
  setDisabled("auto_crop", !caps.auto_crop);
  setDisabled("deskew", !caps.deskew);
}

function setDisabled(id, disabled) {
  const el = $(id);
  el.disabled = disabled;
  if (disabled) el.checked = false;
}

function nearest(values, target) {
  return values.reduce((a, b) => (Math.abs(b - target) < Math.abs(a - target) ? b : a));
}

// --- settings persistence (per browser) ------------------------------------

function loadSettings() {
  try {
    return JSON.parse(localStorage.getItem(SETTINGS_KEY)) || {};
  } catch {
    return {};
  }
}

function saveSettings(s) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
  } catch {
    /* storage unavailable: settings just aren't remembered */
  }
}

function restoreForm() {
  const s = loadSettings();
  if (s.mode) {
    const r = document.querySelector(`input[name="mode"][value="${s.mode}"]`);
    if (r) r.checked = true;
  }
  if (s.format) $("format").value = s.format;
  if (s.quality) $("quality").value = s.quality;
  $("quality-out").textContent = $("quality").value;
  for (const k of ["duplex", "auto_crop", "deskew"]) if (k in s) $(k).checked = s[k];
  if (s.name) $("name").value = s.name;
}

function readForm() {
  return {
    device: $("device").value,
    mode: document.querySelector('input[name="mode"]:checked')?.value || "color",
    resolution: Number($("resolution").value) || 300,
    format: $("format").value,
    quality: Number($("quality").value),
    duplex: $("duplex").checked,
    auto_crop: $("auto_crop").checked,
    deskew: $("deskew").checked,
    name: $("name").value.trim() || "scan",
  };
}

// --- jobs ------------------------------------------------------------------

function isActive(job) {
  return job.state === "queued" || job.state === "scanning";
}

function updateStartButton() {
  const busy = state.jobs.some(isActive);
  const hasDevice = state.devices.length > 0 && !$("device").disabled;
  $("start").disabled = !hasDevice;
  $("start").textContent = busy ? "Queue another scan" : "Start scan";
  $("status").textContent = busy ? "Scanning…" : "";
}

async function startScan(event) {
  event.preventDefault();
  $("form-error").textContent = "";
  const body = readForm();
  saveSettings(body);
  $("start").disabled = true;
  try {
    await api("/api/jobs", { method: "POST", body: JSON.stringify(body) });
    await refreshJobs();
  } catch (e) {
    $("form-error").textContent = e.message;
  } finally {
    updateStartButton();
  }
}

async function cancelJob(id) {
  try {
    await api(`/api/jobs/${id}/cancel`, { method: "POST" });
  } catch (e) {
    $("form-error").textContent = e.message;
  }
  await refreshJobs();
}

async function refreshJobs() {
  try {
    state.jobs = await api("/api/jobs");
  } catch {
    return; // transient; the next poll retries
  }
  renderJobs();
  updateStartButton();

  // Refresh the file list whenever a job finishes.
  const finished = new Set(state.jobs.filter((j) => !isActive(j)).map((j) => j.id));
  const newlyFinished = [...finished].some((id) => !state.lastFinishedIds.has(id));
  state.lastFinishedIds = finished;
  if (newlyFinished) refreshFiles();

  schedulePoll();
}

function schedulePoll() {
  clearTimeout(state.pollTimer);
  const delay = state.jobs.some(isActive) ? 1000 : 5000;
  state.pollTimer = setTimeout(refreshJobs, delay);
}

function renderJobs() {
  const list = $("jobs");
  list.innerHTML = "";
  $("jobs-empty").hidden = state.jobs.length > 0;
  for (const job of state.jobs.slice(0, 8)) {
    const li = document.createElement("li");
    if (isActive(job)) li.append(el("span", "spinner"));

    const info = el("div", "grow");
    const pages = `${job.pages} page${job.pages === 1 ? "" : "s"}`;
    const title = {
      queued: "Waiting…",
      scanning: job.cancel_requested ? `Stopping… ${pages}` : `Scanning… ${pages}`,
      done: `Scanned ${pages}`,
      failed: job.pages ? `Stopped after ${pages}` : "Scan failed",
      cancelled: `Cancelled · ${pages}`,
    }[job.state];
    info.append(el("div", "title", title));
    const sides = job.duplex ? "both sides" : "one side";
    info.append(
      el("div", "meta", `${time(job.created)} · ${job.resolution} dpi · ${job.mode} · ${sides} · ${job.format.toUpperCase()}`),
    );
    if (job.error) info.append(el("div", "job-error", job.error));
    li.append(info);

    if (isActive(job) && !job.cancel_requested) {
      const btn = el("button", "ghost", "Cancel");
      btn.type = "button";
      btn.onclick = () => cancelJob(job.id);
      li.append(btn);
    } else {
      li.append(el("span", `badge ${job.state}`, job.state));
    }
    list.append(li);
  }
}

// --- files -----------------------------------------------------------------

async function refreshFiles() {
  let files;
  try {
    files = await api("/api/files");
  } catch {
    return;
  }
  const list = $("files");
  list.innerHTML = "";
  $("files-empty").hidden = files.length > 0;
  for (const f of files) {
    const li = document.createElement("li");
    const info = el("div", "grow");
    info.append(el("div", "title", f.name));
    info.append(el("div", "meta", `${time(f.modified)} · ${size(f.size)}`));
    const actions = el("div", "actions");
    const view = el("a", "", "Open");
    view.href = `${f.url}?inline=true`;
    view.target = "_blank";
    view.rel = "noopener";
    const dl = el("a", "dl", "Download");
    dl.href = f.url;
    dl.setAttribute("download", f.name);
    actions.append(view, dl);
    li.append(info, actions);
    list.append(li);
  }
}

// --- helpers ---------------------------------------------------------------

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

function time(iso) {
  const d = new Date(iso);
  const sameDay = d.toDateString() === new Date().toDateString();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
}

function size(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

// --- wiring ----------------------------------------------------------------

$("rescan").onclick = loadDevices;
$("device").onchange = () => {
  saveSettings({ ...loadSettings(), device: $("device").value });
  loadCapabilities();
};
$("quality").oninput = () => ($("quality-out").textContent = $("quality").value);
$("scan-form").onsubmit = startScan;

restoreForm();
refreshJobs();
refreshFiles();
loadDevices();

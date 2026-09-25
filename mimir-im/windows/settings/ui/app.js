// The settings window: the macOS settings (SettingsWindow.swift), tab for
// tab, over the commands in src/app.rs. `rpc` reaches the window's own
// engine (get_config / set_config, the learning and the history); the
// dict_* commands edit user_dicts/ as files.

"use strict";

const invoke = (cmd, args = {}) => window.__TAURI__.core.invoke(cmd, args);
const rpc = (method, params = {}) => invoke("rpc", { method, params });

// --- Elements -----------------------------------------------------------------

/** `h("button", { onclick }, "text")`: an element, its properties, its children. */
function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else if (key in el) el[key] = value;
    else el.setAttribute(key, value);
  }
  el.append(...children.flat().filter((c) => c != null && c !== false));
  return el;
}

function showError(text) {
  const el = document.getElementById("error");
  el.textContent = text ?? "";
  el.hidden = !text;
}

/** Ask before a destructive step; resolves to whether to go on. */
function confirmDialog(title, message) {
  const dialog = document.getElementById("confirm");
  document.getElementById("confirm-title").textContent = title;
  document.getElementById("confirm-message").textContent = message;
  dialog.showModal();
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "ok"), { once: true });
  });
}

// --- Settings (get_config / set_config) -------------------------------------

/** Each field of get_config and where set_config writes it. */
const FIELDS = {
  part_hint: ["display", "part_hint"],
  candidate_layout: ["display", "candidate_layout"],
  candidate_window: ["display", "candidate_window"],
  reading_display: ["display", "reading_display"],
  tail_delay_ms: ["display", "tail_delay_ms"],
  tail_delete_delay_ms: ["display", "tail_delete_delay_ms"],
  predictive: ["conversion", "predictive"],
  skk_style: ["conversion", "skk_style"],
  model: ["conversion", "model"],
  light_model: ["conversion", "light_model"],
  learning: ["learning", "enabled"],
  date: ["date", "enabled"],
};

const settings = {
  config: null,
  /** Values as last read or written, by `section.key`, so an unchanged
   *  value isn't written back. */
  saved: {},

  async load() {
    try {
      this.config = await rpc("get_config");
    } catch {
      this.config = null;
      showError("設定を読めませんでした（エンジンが起動していない可能性があります）");
      return;
    }
    this.saved = {};
    for (const [field, [section, key]] of Object.entries(FIELDS)) {
      this.saved[`${section}.${key}`] = String(this.config[field]);
    }
    for (const [action, value] of Object.entries(this.config.keys ?? {})) {
      this.saved[`keys.${action}`] = value;
    }
    showError(null);
  },

  /** Write `[section] key = value` (`null` removes it); whether it took. */
  async set(section, key, value) {
    const id = `${section}.${key}`;
    const text = value == null ? null : String(value);
    if (this.saved[id] === text) return true;
    try {
      await rpc("set_config", { section, key, value });
    } catch {
      showError(`[${section}] ${key} を保存できませんでした`);
      await this.load();
      render();
      return false;
    }
    this.saved[id] = text;
    showError(null);
    return true;
  },

  /** Set one of FIELDS. */
  async field(name, value) {
    this.config[name] = value;
    const [section, key] = FIELDS[name];
    return this.set(section, key, value);
  },
};

// --- Tabs --------------------------------------------------------------------------

const TABS = {
  general: renderGeneral,
  keys: renderKeys,
  models: renderModels,
  dict: renderDict,
  learning: (page) => renderStore(page, "learning", "学習"),
  history: (page) => renderStore(page, "history", "履歴"),
};

let current = "general";

function render() {
  for (const tab of document.querySelectorAll("#tabs button")) {
    tab.setAttribute("aria-selected", String(tab.dataset.tab === current));
  }
  const page = document.getElementById("page");
  page.replaceChildren();
  if (settings.config || current === "dict") TABS[current](page);
}

function section(title, ...rows) {
  return h("section", {}, title && h("h2", {}, title), ...rows);
}

function toggle(label, checked, onchange) {
  return h("div", { class: "row check" },
    h("label", {}, h("input", { type: "checkbox", checked, onchange: (e) => onchange(e.target.checked) }), label));
}

function choice(label, value, options, onchange) {
  return h("div", { class: "row" }, h("label", {}, label),
    h("span", { class: "select" },
      h("select", { onchange: (e) => onchange(e.target.value) },
        options.map(([v, text]) => h("option", { value: v, selected: v === value }, text)))));
}

function stepper(label, value, min, max, step, onchange) {
  return h("div", { class: "row" }, h("label", {}, label),
    h("span", {},
      h("input", {
        type: "number", value, min, max, step,
        onchange: (e) => {
          const n = Math.min(max, Math.max(min, Math.round(Number(e.target.value) / step) * step));
          e.target.value = n;
          onchange(n);
        },
      }),
      " ミリ秒"));
}

/** 操作・表示・変換, as the macOS 一般 tab. */
function renderGeneral(page) {
  const c = settings.config;
  const set = (name) => (value) => settings.field(name, value);
  const tail = c.reading_display === "tail";
  page.append(
    section("操作",
      toggle("SKK 式の入力（大文字で漢字の変換を始め、小文字は平仮名で確定）", c.skk_style, set("skk_style"))),
    section("表示",
      choice("変換中の読み", c.reading_display, [
        ["tail", "末尾1字を平仮名で残す"],
        ["ruby", "ふりがな（最後の漢字の上）"],
        ["inline", "読みを添える（天気〈てんき〉）"],
        ["none", "表示しない"],
      ], async (value) => { await settings.field("reading_display", value); render(); }),
      tail && stepper("手を止めてから変換するまで", c.tail_delay_ms, 50, 1000, 50, set("tail_delay_ms")),
      tail && stepper("削除のあと変換するまで", c.tail_delete_delay_ms, 50, 2000, 50, set("tail_delete_delay_ms")),
      choice("候補窓を出す", c.candidate_window, [
        ["conversion", "変換を始めてから（Ctrl+N など）"],
        ["always", "入力中から常に"],
      ], set("candidate_window")),
      choice("候補窓の並び", c.candidate_layout, [
        ["vertical", "縦に一列"],
        ["keyboard", "キーの位置どおり（2段）"],
      ], set("candidate_layout")),
      toggle("補完候補を表示する（Tab で補完）", c.part_hint === "completion",
        (on) => settings.field("part_hint", on ? "completion" : "none"))),
    section("変換",
      toggle("予測候補を候補窓に出す", c.predictive, set("predictive")),
      toggle("変換結果を学習する", c.learning, set("learning")),
      toggle("きょう・いま などを日付・時刻に変換する", c.date, set("date"))));
}

// --- Keys --------------------------------------------------------------------------

const ACTIONS = [
  ["convert", "候補窓を開く"], ["next", "次の候補"], ["prev", "前の候補"],
  ["close", "パートを閉じて確定"], ["commit", "すべて確定"], ["cancel", "取り消す"],
  ["katakana", "カタカナで確定"], ["complete", "以前の入力で補完"],
  ["left", "変換範囲を縮める"], ["right", "変換範囲を伸ばす"], ["home", "先頭の1文字だけ"],
  ["end", "全体"], ["end_commit", "末尾ですべて確定"], ["register", "単語登録"], ["discard", "入力を捨てる"],
  ["kana", "かな入力へ"], ["ascii", "英数（直接入力）へ"], ["abbrev", "英字のまま変換"],
];

/** Browser key names → the [keys] notation's. */
const NAMED = {
  " ": "Space", Tab: "Tab", Enter: "Return", Escape: "Escape", Backspace: "BackSpace",
  Delete: "Delete", ArrowLeft: "Left", ArrowRight: "Right", ArrowDown: "Down", ArrowUp: "Up",
  Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown",
};

/** The [keys] notation's names → what the keyboard calls them here. */
const SHOWN = {
  Space: "Space", Return: "Enter", Escape: "Esc", Tab: "Tab", BackSpace: "Backspace",
  Delete: "Delete", Left: "←", Right: "→", Up: "↑", Down: "↓", Home: "Home", End: "End",
  PageUp: "PgUp", PageDown: "PgDn",
};

/** `event` in the [keys] notation (C-n, S-Tab, Space, q); `null` for what
 *  can't be bound (Alt / Win chords, a lone modifier, IME input) — or,
 *  `charactersOnly`, anything but a plain character. */
function keySpec(event, charactersOnly) {
  if (event.altKey || event.metaKey || event.isComposing || event.key === "Process") return null;
  if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return null;
  const ctrl = event.ctrlKey ? "C-" : "";
  const shift = event.shiftKey ? "S-" : "";
  const name = NAMED[event.key];
  if (name) return charactersOnly ? null : ctrl + shift + name;
  const c = event.key;
  if (c.length !== 1 || c < "!" || c > "~") return null;
  if (charactersOnly) return ctrl ? null : c;
  // A letter with Shift is S-a; other characters carry their shift (? rather than S-/).
  if (/[a-z]/i.test(c)) return ctrl + shift + c.toLowerCase();
  return ctrl + c;
}

/** C-n → Ctrl+N, S-Tab → Shift+Tab. */
function shownKey(key) {
  let rest = key;
  let prefix = "";
  for (;;) {
    if (rest.startsWith("C-") && rest.length > 2) { prefix += "Ctrl+"; rest = rest.slice(2); }
    else if (rest.startsWith("S-") && rest.length > 2) { prefix += "Shift+"; rest = rest.slice(2); }
    else break;
  }
  return prefix + (SHOWN[rest] ?? (rest.length === 1 ? rest.toUpperCase() : rest));
}

/** Takes the next key presses while armed. */
const recorder = {
  action: null,
  taken: [],
  wanted: 1,
  done: null,

  record(action, count, done) {
    this.stop();
    Object.assign(this, { action, wanted: count, done, taken: [] });
    render();
  },

  stop() {
    Object.assign(this, { action: null, taken: [], done: null });
  },
};

document.addEventListener("keydown", (event) => {
  if (!recorder.action) return;
  event.preventDefault();
  event.stopPropagation();
  const spec = keySpec(event, recorder.wanted > 1);
  if (spec == null) return;
  recorder.taken.push(spec);
  if (recorder.taken.length < recorder.wanted) {
    render();
    return;
  }
  const { taken, done } = recorder;
  recorder.stop();
  done(taken);
}, true);

function boundKeys(action) {
  return (settings.config.keys?.[action] ?? "").split(" ").filter(Boolean);
}

async function saveKeys(action, keys) {
  const value = keys.join(" ");
  settings.config.keys[action] = value;
  if (!(await settings.set("keys", action, value))) return;
  render();
}

function stopButton() {
  return h("button", { onclick: () => { recorder.stop(); render(); } }, "やめる");
}

/** [keys]: each action's keys as chips; × drops one, ＋ records the next
 *  key pressed. The select keys are recorded afresh, eight in a row. */
function renderKeys(page) {
  const select = recorder.action === "select"
    ? [h("span", { class: "recording" }, `キーを8つ順に押す… ${recorder.taken.join("")}`), stopButton()]
    : [h("span", { class: "mono" }, settings.config.keys?.select ?? ""),
       h("button", { onclick: () => recorder.record("select", 8, (keys) => saveKeys("select", [keys.join("")])) }, "記録し直す")];
  page.append(
    section(null, h("div", { class: "row" }, h("span", {}, "候補を選ぶキー"), ...select)),
    section(null,
      ACTIONS.map(([action, label]) => {
        const keys = boundKeys(action);
        const chips = keys.map((key) => h("span", { class: "chip" }, shownKey(key),
          h("button", { title: "外す", "aria-label": `${shownKey(key)} を外す`, onclick: () => saveKeys(action, keys.filter((k) => k !== key)) }, "×")));
        const tail = recorder.action === action
          ? [h("span", { class: "recording" }, "キーを押す…"), stopButton()]
          : [h("button", {
              class: "plain", title: "キーを足す", "aria-label": `${label}にキーを足す`,
              onclick: () => recorder.record(action, 1, (taken) => saveKeys(action, keys.filter((k) => !taken.includes(k)).concat(taken))),
            }, "＋")];
        return h("div", { class: "row" }, h("span", {}, label), h("span", { class: "keys" }, chips, tail));
      }),
      h("p", { class: "muted" }, "＋ を押してから、割り当てたいキーを押す。× で外す。")));
}

// --- Models ----------------------------------------------------------------------

const profiler = { running: false, scores: [], samples: "" };

function currentProfile() {
  const c = settings.config;
  return Object.keys(c.profiles ?? {}).find((name) =>
    c.profiles[name].model === c.model && c.profiles[name].light_model === c.light_model);
}

async function runProfiler() {
  profiler.running = true;
  profiler.scores = [];
  render();
  try {
    const lines = await invoke("profile_models");
    profiler.scores = lines;
    const samples = lines.find((l) => l.samples)?.samples;
    profiler.samples = samples === "learning" ? "あなたの最近の確定（最大50件）" : "内蔵の例文（20件）";
  } catch (e) {
    showError(`計測できませんでした: ${e}`);
  }
  profiler.running = false;
  if (current === "models") render();
}

function renderModels(page) {
  const c = settings.config;
  const profile = currentProfile();
  const models = (c.models ?? []).map((m) => [m, m]);
  const name = h("input", { type: "text", placeholder: "名前" });
  const save = h("button", { disabled: true }, "今の組み合わせを保存");
  name.addEventListener("input", () => { save.disabled = !name.value || name.value.includes("."); });
  save.addEventListener("click", async () => {
    const section = `profiles.${name.value}`;
    if (!(await settings.set(section, "model", c.model)) || !(await settings.set(section, "light_model", c.light_model))) return;
    c.profiles = { ...c.profiles, [name.value]: { model: c.model, light_model: c.light_model } };
    render();
  });
  page.append(
    section("組み合わせ",
      choice("プロファイル", profile ?? "",
        [["", "（保存していない組み合わせ）"], ...Object.keys(c.profiles ?? {}).sort().map((n) => [n, n])],
        async (value) => {
          if (!value) return;
          const p = c.profiles[value];
          await settings.field("model", p.model);
          await settings.field("light_model", p.light_model);
          render();
        }),
      choice("メイン", c.model, models, async (value) => { await settings.field("model", value); render(); }),
      choice("軽量（ビーム・自動切替）", c.light_model, models, async (value) => { await settings.field("light_model", value); render(); }),
      h("div", { class: "bar" }, name, save, h("span", { class: "spacer" }),
        profile && h("button", {
          onclick: async () => {
            const section = `profiles.${profile}`;
            if (!(await settings.set(section, "model", null)) || !(await settings.set(section, "light_model", null))) return;
            delete c.profiles[profile];
            render();
          },
        }, `「${profile}」を削除`))),
    section(null,
      h("div", { class: "bar" }, h("h2", {}, "計測"), h("span", { class: "spacer" }),
        h("button", { disabled: profiler.running, onclick: runProfiler }, profiler.running ? "計測中…" : "計測する")),
      profiler.scores.length === 0
        ? h("p", { class: "muted" }, profiler.running ? "計測中…（数秒〜十数秒）" : "各モデルの精度と速さを、この PC で測ります。")
        : [h("div", { class: "table" }, h("table", {},
            h("thead", {}, h("tr", {}, h("th", {}, "モデル"), h("th", { class: "num" }, "精度"), h("th", { class: "num" }, "1回あたり"), h("th", { class: "num" }, "読込"))),
            h("tbody", {}, profiler.scores.map((s) => h("tr", {},
              h("td", { class: s.model === c.model ? "strong" : "" }, s.model ?? "?"),
              s.error
                ? h("td", { colSpan: 3, class: "muted" }, s.error)
                : [h("td", { class: "num" }, `${Math.round((s.accuracy ?? 0) * 100)}%`),
                   h("td", { class: "num" }, `${(s.avg_ms ?? 0).toFixed(1)} ms`),
                   h("td", { class: "num" }, `${Math.round(s.load_ms ?? 0)} ms`)]))))),
           h("p", { class: "muted" }, `対象: ${profiler.samples}`)]));
}

// --- Tables ------------------------------------------------------------------------

/** A table with a pick column; `sort` (optional) makes the headers sort.
 *  `columns`: [{ title, value(row), text?(row), num? }]. */
function pickTable({ columns, rows, id, selection, sort, onchange }) {
  if (sort) {
    const col = columns.find((c) => c.title === sort.by);
    const dir = sort.desc ? -1 : 1;
    rows = [...rows].sort((a, b) => {
      const x = col.value(a), y = col.value(b);
      return (x < y ? -1 : x > y ? 1 : 0) * dir;
    });
  }
  const all = rows.length > 0 && rows.every((r) => selection.has(id(r)));
  const header = columns.map((col) => {
    const mark = sort?.by === col.title ? (sort.desc ? " ▼" : " ▲") : "";
    return h("th", {
      class: [col.num && "num", sort && "sortable"].filter(Boolean).join(" "),
      onclick: sort && (() => { onchange({ sort: { by: col.title, desc: sort.by === col.title ? !sort.desc : false } }); }),
    }, col.title + mark);
  });
  const body = rows.map((row) => {
    const key = id(row);
    const picked = selection.has(key);
    const flip = () => {
      if (picked) selection.delete(key); else selection.add(key);
      onchange({});
    };
    return h("tr", { class: picked ? "selected" : "", onclick: flip },
      h("td", { class: "pick" }, h("input", { type: "checkbox", checked: picked, "aria-label": "選ぶ", onclick: (e) => e.stopPropagation(), onchange: flip })),
      columns.map((col) => h("td", { class: col.num ? "num" : "" }, (col.text ?? col.value)(row))));
  });
  return h("div", { class: "table fill" }, h("table", {},
    h("thead", {}, h("tr", {},
      h("th", { class: "pick" }, h("input", {
        type: "checkbox", checked: all, "aria-label": "すべて選ぶ",
        onchange: (e) => {
          for (const r of rows) {
            if (e.target.checked) selection.add(id(r)); else selection.delete(id(r));
          }
          onchange({});
        },
      })),
      header)),
    h("tbody", {}, body)));
}

function search(query, onquery) {
  const input = h("input", { type: "search", placeholder: "検索", value: query });
  input.addEventListener("input", () => onquery(input.value));
  return input;
}

/** Re-render a tab keeping the search box focused, caret and all. */
function rerenderKeepingFocus() {
  const focused = document.activeElement;
  const at = focused?.type === "search" ? focused.selectionStart : null;
  render();
  if (at != null) {
    const input = document.querySelector("#page input[type=search]");
    input.focus();
    input.setSelectionRange(at, at);
  }
}

// --- Dictionary ------------------------------------------------------------------

const dict = { entries: [], binaries: [], selection: new Set(), query: "", loaded: false };

async function loadDict() {
  try {
    Object.assign(dict, await invoke("dict_list"), { loaded: true });
    dict.selection.clear();
  } catch (e) {
    showError(String(e));
  }
}

function renderDict(page) {
  if (!dict.loaded) {
    loadDict().then(() => current === "dict" && render());
    return;
  }
  const reading = h("input", { type: "text", placeholder: "よみ（ひらがな）" });
  const word = h("input", { type: "text", placeholder: "単語" });
  const add = h("button", { disabled: true }, "追加");
  const ready = () => { add.disabled = !reading.value || !word.value; };
  reading.addEventListener("input", ready);
  word.addEventListener("input", ready);
  add.addEventListener("click", async () => {
    try {
      await invoke("dict_add", { reading: reading.value, word: word.value });
    } catch (e) {
      showError(String(e));
      return;
    }
    await loadDict();
    render();
  });
  const q = dict.query;
  const rows = q ? dict.entries.filter((e) => e.reading.includes(q) || e.word.includes(q)) : dict.entries;
  const id = (e) => `${e.file}:${e.line}`;
  page.append(
    h("div", { class: "bar" }, reading, word, add),
    pickTable({
      columns: [
        { title: "よみ", value: (e) => e.reading },
        { title: "単語", value: (e) => e.word },
        { title: "ファイル", value: (e) => e.file },
      ],
      rows, id, selection: dict.selection, onchange: () => render(),
    }),
    h("div", { class: "bar" },
      search(q, (value) => { dict.query = value; rerenderKeepingFocus(); }),
      h("span", { class: "muted num" }, `${rows.length} 件`),
      h("span", { class: "spacer" }),
      h("button", { onclick: () => invoke("dict_open").catch((e) => showError(String(e))) }, "フォルダを開く"),
      h("button", {
        disabled: dict.selection.size === 0,
        onclick: async () => {
          const doomed = dict.entries.filter((e) => dict.selection.has(id(e))).map(({ file, line }) => ({ file, line }));
          try {
            await invoke("dict_remove", { entries: doomed });
          } catch (e) {
            showError(String(e));
          }
          await loadDict();
          render();
        },
      }, "選択を削除")),
    dict.binaries.length > 0 &&
      h("p", { class: "muted" }, `バイナリ辞書（ここでは編集できません）: ${dict.binaries.join(", ")}`));
}

// --- Learning and history (held by the engine) --------------------------------------

/** One of the engine's two stores: `learning` (what reranks conversion) or
 *  `history` (every commit, what completion draws from). */
const stores = {};

function store(name) {
  stores[name] ??= { entries: null, selection: new Set(), query: "", sort: { by: "最後に確定", desc: true } };
  return stores[name];
}

async function loadStore(name) {
  const s = store(name);
  try {
    s.entries = (await rpc(`${name}_list`)).entries;
  } catch (e) {
    s.entries = [];
    showError(String(e));
  }
  s.selection.clear();
}

const pad = (n) => String(n).padStart(2, "0");

function dateText(seconds) {
  const d = new Date(seconds * 1000);
  return `${d.getFullYear()}/${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** The 学習 and 履歴 tabs: one store's entries, newest first by default. */
function renderStore(page, name, noun) {
  const s = store(name);
  if (!s.entries) {
    loadStore(name).then(() => current === name && render());
    return;
  }
  const q = s.query;
  const rows = q ? s.entries.filter((e) => e.reading.includes(q) || e.surface.includes(q)) : s.entries;
  const id = (e) => `${e.reading}\t${e.surface}`;
  page.append(
    pickTable({
      columns: [
        { title: "よみ", value: (e) => e.reading },
        { title: "確定した語", value: (e) => e.surface },
        { title: "回数", value: (e) => e.frequency, num: true },
        { title: "最後に確定", value: (e) => e.last_access, text: (e) => dateText(e.last_access), num: true },
      ],
      rows, id, selection: s.selection, sort: s.sort,
      onchange: ({ sort }) => { if (sort) s.sort = sort; render(); },
    }),
    h("div", { class: "bar" },
      search(q, (value) => { s.query = value; rerenderKeepingFocus(); }),
      h("span", { class: "muted num" }, `${rows.length} 件`),
      h("span", { class: "spacer" }),
      h("button", {
        disabled: s.selection.size === 0,
        onclick: async () => {
          for (const e of s.entries.filter((e) => s.selection.has(id(e)))) {
            await rpc(`${name}_remove`, { reading: e.reading, surface: e.surface }).catch((err) => showError(String(err)));
          }
          await loadStore(name);
          render();
        },
      }, "選択を削除"),
      h("button", {
        class: "danger",
        disabled: s.entries.length === 0,
        onclick: async () => {
          if (!(await confirmDialog(`${noun}をすべて消しますか？`, `${s.entries.length} 件の${noun}が消え、元に戻せません。`))) return;
          await rpc(`${name}_clear`).catch((err) => showError(String(err)));
          await loadStore(name);
          render();
        },
      }, "すべてリセット…")));
}

// --- Start -------------------------------------------------------------------------

for (const tab of document.querySelectorAll("#tabs button")) {
  tab.addEventListener("click", async () => {
    current = tab.dataset.tab;
    recorder.stop();
    // Other applications' engines write the learning and the dictionaries
    // too: read them afresh on every visit.
    if (current === "dict") dict.loaded = false;
    if (stores[current]) stores[current].entries = null;
    render();
  });
}

settings.load().then(render);

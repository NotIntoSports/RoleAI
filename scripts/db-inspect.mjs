import { DatabaseSync } from "node:sqlite";
import { join } from "node:path";
import { homedir } from "node:os";

const dbPath = join(homedir(), "AppData", "Roaming", "com.aivirtualassistant.desktop", "app.sqlite3");
const readOnly = process.argv.includes("--write") ? undefined : true;
const db = new DatabaseSync(dbPath, { readOnly });

const command = process.argv[2] ?? "inspect";
if (command === "inject") {
  const writable = new DatabaseSync(dbPath);
  const latest = writable.prepare("SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1").get();
  if (!latest) throw new Error("no session");
  const insert = writable.prepare(
    "INSERT INTO session_turns(id, session_id, turn_index, user_text, assistant_text, materials_used, created_at) VALUES (?,?,?,?,?,?,?)",
  );
  const now = new Date().toISOString();
  const rows = [
    ["ui-test-turn-1", 900, "第一轮测试：现在什么时间？", "第一轮测试回答：现在是下午。"],
    ["ui-test-turn-2", 901, "第二轮测试：我上一句问了什么？", "第二轮测试回答：你问了时间。"],
    ["ui-test-turn-3", 902, "第三轮测试：请重复第一轮的问题。", "第三轮测试回答：第一轮问的是时间。"],
  ];
  for (const [id, index, userText, assistantText] of rows) {
    insert.run(id, latest.id, index, userText, assistantText, 0, now);
  }
  console.log("injected into", latest.id);
  writable.close();
} else if (command === "clean") {
  const writable = new DatabaseSync(dbPath);
  writable.prepare("DELETE FROM session_turns WHERE id LIKE 'ui-test-turn-%'").run();
  console.log("cleaned");
  writable.close();
} else if (command === "inspect") {
  const tables = db.prepare("SELECT name FROM sqlite_master WHERE type='table'").all();
  console.log("tables:", tables.map((t) => t.name).join(","));
  const sessions = db.prepare("SELECT id,status,started_at,role_profile_id FROM sessions ORDER BY started_at DESC LIMIT 5").all();
  console.log(JSON.stringify(sessions, null, 1));
  const latest = sessions[0];
  if (latest) {
    const turns = db.prepare("SELECT id,turn_index,user_text,assistant_text FROM session_turns WHERE session_id=? ORDER BY turn_index").all(latest.id);
    console.log(`turns of ${latest.id}:`, JSON.stringify(turns, null, 1));
  }
}

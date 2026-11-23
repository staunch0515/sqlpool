import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "./api";
import { QueryResult } from "./types";

/**
 * Writes a result as CSV into the export folder. With no folder set yet, asks once and remembers it
 * through `onExportDir`. Returns the file written, or null when the user cancelled the choice.
 */
export async function exportResult(
  result: QueryResult,
  title: string,
  exportDir: string,
  onExportDir: (dir: string) => void,
): Promise<string | null> {
  let dir = exportDir.trim();
  if (!dir) {
    const chosen = await open({ directory: true, multiple: false, title: "Choose the folder for CSV exports" });
    if (typeof chosen !== "string") return null;
    dir = chosen;
    onExportDir(dir);
  }
  // Written straight into the folder, with a time in the name so nothing is overwritten.
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const file = `${title.replace(/[\\/:*?"<>|]/g, "-")} ${stamp}.csv`;
  const path = `${dir.replace(/\/+$/, "")}/${file}`;
  await invoke("export_result_csv", {
    path,
    result: { columns: result.columns, rows: result.rows, truncated: result.truncated },
  });
  return path;
}

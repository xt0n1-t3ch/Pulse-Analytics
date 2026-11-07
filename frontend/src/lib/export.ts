export interface ExportDateConfig {
    includeDate: boolean;
    includeTime: boolean;
    dateFormat: "yyyy-mm-dd" | "dd/mm/yyyy" | "mm/dd/yyyy";
    timeFormat: "24h" | "12h";
    includeSeconds: boolean;
}

export interface ExportColumn {
    key: string;
    label: string;
    enabled: boolean;
}

export type ExportFormat = "csv" | "json" | "tsv";

const DEFAULT_DATE_CONFIG: ExportDateConfig = {
    includeDate: true,
    includeTime: true,
    dateFormat: "yyyy-mm-dd",
    timeFormat: "24h",
    includeSeconds: false,
};

export function defaultDateConfig(): ExportDateConfig {
    return { ...DEFAULT_DATE_CONFIG };
}

export function formatExportDate(
    isoOrTime: string | null,
    config: ExportDateConfig,
): string {
    if (!isoOrTime) return "";
    const date = isoOrTime.includes("T") ? new Date(isoOrTime) : null;
    if (!date && isoOrTime.match(/^\d{2}:\d{2}/)) {
        if (!config.includeTime) return "";
        return isoOrTime;
    }
    if (!date || isNaN(date.getTime())) return isoOrTime;

    const parts: string[] = [];

    if (config.includeDate) {
        const y = date.getFullYear();
        const m = String(date.getMonth() + 1).padStart(2, "0");
        const d = String(date.getDate()).padStart(2, "0");
        if (config.dateFormat === "yyyy-mm-dd") parts.push(`${y}-${m}-${d}`);
        else if (config.dateFormat === "dd/mm/yyyy")
            parts.push(`${d}/${m}/${y}`);
        else parts.push(`${m}/${d}/${y}`);
    }

    if (config.includeTime) {
        let h = date.getHours();
        const min = String(date.getMinutes()).padStart(2, "0");
        const sec = String(date.getSeconds()).padStart(2, "0");
        let suffix = "";
        if (config.timeFormat === "12h") {
            suffix = h >= 12 ? " PM" : " AM";
            h = h % 12 || 12;
        }
        const hStr = String(h).padStart(2, "0");
        parts.push(
            config.includeSeconds
                ? `${hStr}:${min}:${sec}${suffix}`
                : `${hStr}:${min}${suffix}`,
        );
    }

    return parts.join(" ");
}

function escapeCSV(val: string): string {
    if (val.includes(",") || val.includes('"') || val.includes("\n")) {
        return `"${val.replace(/"/g, '""')}"`;
    }
    return val;
}

export function exportToString(
    rows: Record<string, unknown>[],
    columns: ExportColumn[],
    format: ExportFormat,
): string {
    const active = columns.filter((c) => c.enabled);

    if (format === "json") {
        const filtered = rows.map((row) => {
            const obj: Record<string, unknown> = {};
            for (const col of active) {
                obj[col.key] = row[col.key];
            }
            return obj;
        });
        return JSON.stringify(filtered, null, 2);
    }

    const sep = format === "tsv" ? "\t" : ",";
    const header = active
        .map((c) => (format === "csv" ? escapeCSV(c.label) : c.label))
        .join(sep);
    const lines = rows.map((row) =>
        active
            .map((col) => {
                const val = String(row[col.key] ?? "");
                return format === "csv" ? escapeCSV(val) : val;
            })
            .join(sep),
    );
    return [header, ...lines].join("\n");
}

export function saveExport(
    content: string,
    format: ExportFormat,
    defaultName: string,
): boolean {
    const ext = format === "tsv" ? "tsv" : format;
    const mime = format === "json" ? "application/json" : "text/plain";
    const blob = new Blob([content], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${defaultName}.${ext}`;
    a.click();
    URL.revokeObjectURL(url);
    return true;
}

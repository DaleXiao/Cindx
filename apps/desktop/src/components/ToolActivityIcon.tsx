import {
  Blocks,
  Camera,
  Command,
  DatabaseZap,
  FilePenLine,
  FileSearch,
  FolderTree,
  GitBranch,
  Globe2,
  ImagePlus,
  Keyboard,
  LibraryBig,
  MonitorCog,
  Mouse,
  MousePointer2,
  MousePointerClick,
  MoveVertical,
  Network,
  PanelTopOpen,
  PanelsTopLeft,
  Plug,
  ScanLine,
  ScanText,
  Search,
  SearchCode,
  SquareTerminal,
  TextCursorInput,
  Users,
  Waypoints,
  Wrench,
  type LucideIcon
} from "lucide-react";

const TOOL_ICONS: Record<string, LucideIcon> = {
  "file.read": FileSearch,
  "file.read_many": FileSearch,
  file_read: FileSearch,
  "file.list": FolderTree,
  "file.search": SearchCode,
  file_search: SearchCode,
  "file.write": FilePenLine,
  "filesystem.write": FilePenLine,
  "shell.run": SquareTerminal,
  shell_run: SquareTerminal,
  "process.run": SquareTerminal,
  "web.search": Search,
  "browser.open": Globe2,
  "browser.extract_text": ScanText,
  "browser.capture": Camera,
  "browser.click": MousePointerClick,
  "browser.type": Keyboard,
  "browser.scroll": MoveVertical,
  "browser.tabs": PanelsTopLeft,
  "browser.select_tab": PanelTopOpen,
  "computer.screenshot": ScanLine,
  "computer.click": MousePointer2,
  "computer.type": TextCursorInput,
  "computer.key": Command,
  "computer.scroll": Mouse,
  "image.generate": ImagePlus,
  graph_recall: Network,
  graph_walk: Waypoints,
  semantic_rag: DatabaseZap,
  "catalog.unique": LibraryBig,
  "tool.invoke": Blocks
};

function fallbackToolIcon(toolName: string): LucideIcon {
  if (toolName.startsWith("file.") || toolName.startsWith("filesystem.")) return FileSearch;
  if (toolName.startsWith("shell.") || toolName.startsWith("process.")) return SquareTerminal;
  if (toolName.startsWith("browser.") || toolName.startsWith("web.")) return Globe2;
  if (toolName.startsWith("computer.")) return MonitorCog;
  if (toolName.startsWith("image.")) return ImagePlus;
  if (toolName.startsWith("graph_")) return Network;
  if (/^(semantic_|rag[._]|memory[._]|knowledge[._])/.test(toolName)) return DatabaseZap;
  if (toolName.startsWith("git.")) return GitBranch;
  if (toolName.startsWith("agent.") || toolName.startsWith("collaboration.")) return Users;
  if (toolName.startsWith("mcp") || toolName.includes("__")) return Plug;
  if (toolName.startsWith("tool.")) return Blocks;
  return Wrench;
}

export function ToolActivityIcon({ toolName }: { toolName?: string | null }) {
  const normalized = toolName?.trim().toLowerCase() ?? "";
  const Icon = TOOL_ICONS[normalized] ?? fallbackToolIcon(normalized);
  return <Icon aria-hidden="true" />;
}

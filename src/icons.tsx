/**
 * The app's icons: Tabler (tablericons.com). One place maps each job to an icon, so a change of
 * icon is made here, not in every component. Colours come from the CSS classes that wrap them.
 */
import {
  IconArrowsMaximize,
  IconArrowsMinimize,
  IconBook,
  IconChevronDown,
  IconChevronRight,
  IconCopy,
  IconCopyPlus,
  IconDatabase,
  IconDownload,
  IconDeviceFloppy,
  IconFileTypeSql,
  IconFilePlus,
  IconFolderFilled,
  IconFolderOpen,
  IconFolderPlus,
  IconHash,
  IconHistory,
  IconInfoCircle,
  IconInfoCircleFilled,
  IconList,
  IconListTree,
  IconPlayerPlay,
  IconPlus,
  IconRefresh,
  IconSettings,
  IconStack2,
  IconSparkles,
  IconSitemap,
  IconChecklist,
  IconStar,
  IconClock,
  IconArrowsDiff,
  IconTable,
  IconScript,
  IconTerminal2,
  IconStarFilled,
  IconTrash,
  IconX,
} from "@tabler/icons-react";

type Props = { size?: number; strokeWidth?: number; className?: string };

/** Every icon is drawn this much larger than its nominal size: the app's icons read small otherwise. */
const SCALE = 1.25;

function wrap(Tabler: typeof IconX) {
  return function Icon({ size = 16, strokeWidth = 1.75, className }: Props) {
    return <Tabler size={Math.round(size * SCALE)} stroke={strokeWidth} className={className} />;
  };
}

export const ChevronRight = wrap(IconChevronRight);
export const ChevronDown = wrap(IconChevronDown);
/** Expand all / collapse all. */
export const ExpandAll = wrap(IconArrowsMaximize);
export const CollapseAll = wrap(IconArrowsMinimize);
export const Folder = wrap(IconFolderFilled);
export const FolderOpen = wrap(IconFolderOpen);
export const FolderPlus = wrap(IconFolderPlus);
export const FileCode = wrap(IconFileTypeSql);
export const FilePlus = wrap(IconFilePlus);
export const Layers = wrap(IconStack2);
export const Plus = wrap(IconPlus);
export const Trash = wrap(IconTrash);
export const Refresh = wrap(IconRefresh);
export const Star = wrap(IconStar);
export const StarFilled = wrap(IconStarFilled);
export const Info = wrap(IconInfoCircle);
export const InfoFilled = wrap(IconInfoCircleFilled);
export const Book = wrap(IconBook);
export const ListTree = wrap(IconListTree);
export const Gear = wrap(IconSettings);
export const Database = wrap(IconDatabase);
export const Download = wrap(IconDownload);
export const Terminal = wrap(IconTerminal2);
export const Script = wrap(IconScript);
export const Sparkles = wrap(IconSparkles);
export const Table = wrap(IconTable);
export const Sitemap = wrap(IconSitemap);
export const Checklist = wrap(IconChecklist);
export const Clock = wrap(IconClock);
export const Diff = wrap(IconArrowsDiff);
export const History = wrap(IconHistory);
export const X = wrap(IconX);
export const Play = wrap(IconPlayerPlay);
export const Save = wrap(IconDeviceFloppy);
export const CopyPlus = wrap(IconCopyPlus);
export const Copy = wrap(IconCopy);
export const Hash = wrap(IconHash);
export const List = wrap(IconList);

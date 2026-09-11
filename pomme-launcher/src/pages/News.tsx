import { openUrl } from "@tauri-apps/plugin-opener";
import { ArrowLeft, ExternalLink } from "lucide-react";
import { PatchNote } from "../bindings/pomme_launcher/commands";
import { useAppStore } from "../lib/store";

const MORE_PATCH_NOTES_URL = "https://aka.ms/MorePatchNotes";
const ICON_SIZE = 14;
const LINK_ICON_SIZE = 12;

export default function NewsPage({
  openPatchNote,
}: {
  openPatchNote: (note: PatchNote) => Promise<void>;
}) {
  const selectedNote = useAppStore((state) => state.selectedNote);
  const setSelectedNote = useAppStore((state) => state.setSelectedNote);
  const news = useAppStore((state) => state.news);

  const hasNews = news.length > 0;

  const openMorePatchNotes = (event: React.MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault();
    openUrl(MORE_PATCH_NOTES_URL);
  };

  if (selectedNote) {
    return (
      <div className="page flex flex-col">
        <button className="button-secondary mb-5 self-start" onClick={() => setSelectedNote(null)}>
          <ArrowLeft size={ICON_SIZE} /> Back
        </button>
        <div className="mb-6 flex items-center gap-5 border-b border-line pb-5">
          <div className="aspect-video w-44 shrink-0 overflow-hidden bg-panel">
            <img
              src={selectedNote.image_url}
              alt={selectedNote.title}
              className="block size-full object-cover"
            />
          </div>
          <div className="flex min-w-0 flex-col gap-2">
            <div className="flex items-center gap-2.5">
              <span className="text-[11px] text-muted tabular-nums">
                {selectedNote.date?.replace(/-/g, ".")}
              </span>
              <span className="size-[3px] shrink-0 bg-line-strong" />
              <span className="text-[9px] font-semibold tracking-[0.14em] text-green uppercase">
                {selectedNote.entry_type}
              </span>
            </div>
            <h2 className="text-[22px] leading-tight font-semibold text-foreground">
              {selectedNote.title}
            </h2>
          </div>
        </div>
        <div className="note-body" dangerouslySetInnerHTML={{ __html: selectedNote.body }} />
      </div>
    );
  }

  return (
    <div className="page flex flex-col">
      <h2 className="page-heading mb-6">NEWS & UPDATES</h2>
      <div className="list">
        {news.map((item) => (
          <div
            className="group flex cursor-pointer gap-5 border-b border-line py-4"
            key={item.version}
            onClick={() => openPatchNote(item)}
          >
            <div className="relative aspect-video w-44 shrink-0 overflow-hidden bg-panel">
              <img
                src={item.image_url}
                alt={item.title}
                className="absolute inset-0 size-full object-cover"
              />
              <span className="news-badge">{item.entry_type}</span>
            </div>
            <div className="flex min-w-0 flex-1 flex-col gap-1.5">
              <div className="flex items-center justify-between">
                <span className="text-[11px] text-muted tabular-nums">
                  {item.date.replace(/-/g, ".")}
                </span>
                <span className="text-xs leading-none text-green opacity-0 transition-opacity duration-75 group-hover:opacity-100">
                  →
                </span>
              </div>
              <h3 className="text-sm leading-[1.35] font-semibold text-foreground transition-colors duration-75 group-hover:text-green">
                {item.title}
              </h3>
              <p className="line-clamp-2 max-w-[72ch] text-xs leading-[1.55] text-muted">
                {item.summary}
              </p>
              <span className="mt-auto text-[11px] text-faint tabular-nums">{item.version}</span>
            </div>
          </div>
        ))}
        {!hasNews && <p className="py-6 text-xs text-faint">Loading patch notes...</p>}
      </div>

      <a
        className="mt-5 inline-flex items-center gap-1.5 self-start text-[10px] font-semibold tracking-[0.12em] text-muted uppercase transition-colors duration-75 hover:text-green"
        href={MORE_PATCH_NOTES_URL}
        onClick={openMorePatchNotes}
      >
        More patch notes <ExternalLink size={LINK_ICON_SIZE} />
      </a>
    </div>
  );
}

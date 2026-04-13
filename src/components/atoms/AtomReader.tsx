import { useState, useEffect, useCallback, ReactNode, useRef, useMemo, memo } from 'react';
import { ChevronDown, Loader2 } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import CodeMirror, { type ReactCodeMirrorRef } from '@uiw/react-codemirror';
import { undo, redo } from '@codemirror/commands';
import { openExternalUrl } from '../../lib/platform';
import { Modal } from '../ui/Modal';
import { SearchBar } from '../ui/SearchBar';
import { Input } from '../ui/Input';
import { MarkdownImage } from '../ui/MarkdownImage';
import { LinkPreview } from '../ui/LinkPreview';
import { TagChip } from '../tags/TagChip';
import { TagSelector } from '../tags/TagSelector';
import { MiniGraphPreview } from '../canvas/MiniGraphPreview';
import { useAtomsStore, type AtomWithTags, type SimilarAtomResult } from '../../stores/atoms';
import { useTagsStore } from '../../stores/tags';
import { useUIStore } from '../../stores/ui';
import { useContentSearch, useInlineEditor } from '../../hooks';
import { formatDate } from '../../lib/date';
import { chunkMarkdown, findChunkIndexForOffset } from '../../lib/markdown';
import { getTransport } from '../../lib/transport';
import { getEditorExtensions } from '../../lib/codemirror-config';
import { readerEditorActions } from '../../lib/reader-editor-bridge';

// Progressive rendering configuration
const CHUNK_SIZE = 8000;
const INITIAL_CHUNKS = 1;
const CHUNKS_PER_BATCH = 2;
const CHUNK_DELAY = 32;

const remarkPluginsStable = [remarkGfm];

const MemoizedMarkdownChunk = memo(function MarkdownChunk({
  content,
  components,
}: {
  content: string;
  components: any;
}) {
  return (
    <ReactMarkdown remarkPlugins={remarkPluginsStable} components={components}>
      {content}
    </ReactMarkdown>
  );
});

interface AtomReaderProps {
  atomId: string;
  highlightText?: string | null;
  initialEditing?: boolean;
}

export function AtomReader({ atomId, highlightText, initialEditing }: AtomReaderProps) {
  const deleteAtom = useAtomsStore(s => s.deleteAtom);
  const fetchTags = useTagsStore(s => s.fetchTags);
  const setSelectedTag = useUIStore(s => s.setSelectedTag);
  const overlayNavigate = useUIStore(s => s.overlayNavigate);
  const overlayDismiss = useUIStore(s => s.overlayDismiss);

  const [atom, setAtom] = useState<AtomWithTags | null>(null);
  const [isLoadingAtom, setIsLoadingAtom] = useState(true);
  const [showLoading, setShowLoading] = useState(false);


  // Watch the atoms store for updates to the currently viewed atom
  const storeAtom = useAtomsStore((s) =>
    s.atoms.find((a) => a.id === atomId)
  );

  // Fetch atom from database
  useEffect(() => {
    setIsLoadingAtom(true);
    setShowLoading(false);

    // Only show loading indicator if fetch takes longer than 200ms
    const loadingTimer = setTimeout(() => setShowLoading(true), 200);

    getTransport().invoke<AtomWithTags | null>('get_atom_by_id', { id: atomId })
      .then((fetchedAtom) => {
        clearTimeout(loadingTimer);
        setAtom(fetchedAtom);
        setIsLoadingAtom(false);
        lastFetchedAt.current = fetchedAtom?.updated_at ?? null;
      })
      .catch((error) => {
        clearTimeout(loadingTimer);
        console.error('Failed to fetch atom:', error);
        setAtom(null);
        setIsLoadingAtom(false);
        // atom loaded
      });

    return () => clearTimeout(loadingTimer);
  }, [atomId]);

  // Re-fetch when store summary changes (e.g., after tag extraction)
  const storeAtomUpdatedAt = storeAtom?.updated_at;
  const lastFetchedAt = useRef<string | null>(null);
  useEffect(() => {
    if (storeAtomUpdatedAt && !isLoadingAtom && storeAtomUpdatedAt !== lastFetchedAt.current) {
      lastFetchedAt.current = storeAtomUpdatedAt;
      getTransport().invoke<AtomWithTags | null>('get_atom_by_id', { id: atomId })
        .then((fetchedAtom) => {
          if (fetchedAtom) setAtom(fetchedAtom);
        })
        .catch(console.error);
    }
  }, [atomId, storeAtomUpdatedAt, isLoadingAtom]);

  return (
    <div className="h-full bg-[var(--color-bg-main)]">
      {isLoadingAtom ? (
        showLoading ? (
          <div className="flex items-center justify-center h-full text-[var(--color-text-secondary)]">
            Loading...
          </div>
        ) : null
      ) : !atom ? (
        <div className="flex items-center justify-center h-full text-[var(--color-text-secondary)]">
          Atom not found
        </div>
      ) : (
        <AtomReaderContent
          atom={atom}
          highlightText={highlightText}
          initialEditing={initialEditing}
          onDismiss={overlayDismiss}
          onDelete={async () => {
            await deleteAtom(atomId);
            await fetchTags();
            overlayDismiss();
          }}
          onTagClick={(tagId) => { setSelectedTag(tagId); overlayDismiss(); }}
          onRelatedAtomClick={(id) => overlayNavigate({ type: 'reader', atomId: id })}
          onViewGraph={() => overlayNavigate({ type: 'graph', atomId })}
          onAtomUpdated={(updated) => setAtom(updated)}
        />
      )}
    </div>
  );
}

interface AtomReaderContentProps {
  atom: AtomWithTags;
  highlightText?: string | null;
  initialEditing?: boolean;
  onDismiss: () => void;
  onDelete: () => Promise<void>;
  onTagClick: (tagId: string) => void;
  onRelatedAtomClick: (atomId: string) => void;
  onViewGraph: () => void;
  onAtomUpdated?: (atom: AtomWithTags) => void;
}

function AtomReaderContent({
  atom, highlightText, initialEditing,
  onDismiss, onDelete, onTagClick, onRelatedAtomClick, onViewGraph, onAtomUpdated,
}: AtomReaderContentProps) {
  const readerTheme = useUIStore(s => s.readerTheme);
  const scrollContainerRef = useRef<HTMLDivElement | null>(null);
  const articleRef = useRef<HTMLElement | null>(null);
  const editorRef = useRef<ReactCodeMirrorRef>(null);
  const [showDeleteModal, setShowDeleteModal] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const [showTagSelector, setShowTagSelector] = useState(false);

  // Inline editor
  const {
    isEditing, isTransitioning, editContent, editSourceUrl, editTags, saveStatus, cursorOffset,
    startEditing, stopEditing, setEditContent, setEditSourceUrl, setEditTags, saveNow,
  } = useInlineEditor({ atom, onAtomUpdated });

  const setReaderEditState = useUIStore(s => s.setReaderEditState);

  // Sync editing state to UI store so MainView titlebar can read it
  useEffect(() => {
    setReaderEditState(isEditing, saveStatus);
  }, [isEditing, saveStatus, setReaderEditState]);

  // Populate bridge ref so MainView can dispatch undo/redo/start/stop
  useEffect(() => {
    readerEditorActions.current = {
      startEditing,
      stopEditing,
      undo: () => { const v = editorRef.current?.view; if (v) undo(v); },
      redo: () => { const v = editorRef.current?.view; if (v) redo(v); },
    };
    return () => { readerEditorActions.current = null; };
  }, [startEditing, stopEditing]);

  // Start in editing mode if requested
  useEffect(() => {
    if (initialEditing) startEditing();
  }, [initialEditing, startEditing]);

  // Fade-in on mount
  const [revealed, setRevealed] = useState(false);
  useEffect(() => {
    const frame = requestAnimationFrame(() => setRevealed(true));
    return () => cancelAnimationFrame(frame);
  }, []);

  // Initial highlight state
  const [initialHighlight, setInitialHighlight] = useState<string | null>(null);
  const [targetChunkIndex, setTargetChunkIndex] = useState<number | null>(null);

  // Progressive rendering
  const chunks = useMemo(() => chunkMarkdown(atom.content, CHUNK_SIZE), [atom.content]);
  const [renderedChunkCount, setRenderedChunkCount] = useState(INITIAL_CHUNKS);
  const isFullyRendered = renderedChunkCount >= chunks.length;

  useEffect(() => {
    if (isFullyRendered) return;
    if ('requestIdleCallback' in window) {
      const id = requestIdleCallback(() => {
        setRenderedChunkCount(prev => Math.min(prev + CHUNKS_PER_BATCH, chunks.length));
      }, { timeout: 100 });
      return () => cancelIdleCallback(id);
    } else {
      const id = setTimeout(() => {
        setRenderedChunkCount(prev => Math.min(prev + CHUNKS_PER_BATCH, chunks.length));
      }, CHUNK_DELAY);
      return () => clearTimeout(id);
    }
  }, [renderedChunkCount, chunks.length, isFullyRendered]);

  useEffect(() => { setRenderedChunkCount(INITIAL_CHUNKS); }, [atom.id]);

  // Calculate target chunk from highlightText
  useEffect(() => {
    if (highlightText && atom.content) {
      const offset = atom.content.indexOf(highlightText);
      if (offset !== -1) {
        const chunkIndex = findChunkIndexForOffset(atom.content, offset, CHUNK_SIZE);
        setTargetChunkIndex(chunkIndex);
        setInitialHighlight(highlightText.slice(0, 50).trim());
      }
    } else {
      setInitialHighlight(null);
      setTargetChunkIndex(null);
    }
  }, [highlightText, atom.content]);

  // Scroll to highlight
  useEffect(() => {
    if (targetChunkIndex === null || initialHighlight === null) return;
    if (targetChunkIndex >= renderedChunkCount) {
      setRenderedChunkCount(targetChunkIndex + 1);
      return;
    }
    const scrollTimeout = setTimeout(() => {
      const mark = document.querySelector('[data-initial-highlight]');
      if (mark && scrollContainerRef.current) {
        mark.scrollIntoView({ behavior: 'smooth', block: 'center' });
      }
    }, 100);
    const clearTimer = setTimeout(() => {
      setInitialHighlight(null);
      setTargetChunkIndex(null);
    }, 5000);
    return () => { clearTimeout(scrollTimeout); clearTimeout(clearTimer); };
  }, [targetChunkIndex, renderedChunkCount, initialHighlight]);

  // Content search
  const {
    isOpen: isSearchOpen, query: searchQuery, searchedQuery,
    currentIndex, totalMatches,
    setQuery: setSearchQuery, openSearch, closeSearch, goToNext, goToPrevious, processChildren,
  } = useContentSearch(atom.content);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Cmd+S: immediate save
      if ((e.ctrlKey || e.metaKey) && e.key === 's') {
        e.preventDefault();
        if (isEditing) saveNow();
        return;
      }
      // Cmd+F: search (only when not editing — CodeMirror handles its own search)
      if ((e.ctrlKey || e.metaKey) && e.key === 'f') {
        if (!isEditing) {
          e.preventDefault();
          openSearch();
        }
        return;
      }
      if (e.key === 'Escape') {
        if (showDeleteModal || isSearchOpen) return;
        // When editing, Escape is handled by the CodeMirror keymap directly
        if (isEditing) return;
        onDismiss();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [openSearch, showDeleteModal, isSearchOpen, onDismiss, isEditing, saveNow, stopEditing]);

  // Highlight helpers
  const highlightInitialText = useCallback((text: string): ReactNode => {
    if (!initialHighlight || !text) return text;
    const idx = text.toLowerCase().indexOf(initialHighlight.toLowerCase());
    if (idx === -1) return text;
    return (
      <>
        {text.slice(0, idx)}
        <mark data-initial-highlight="true" className="initial-highlight">
          {text.slice(idx, idx + initialHighlight.length)}
        </mark>
        {text.slice(idx + initialHighlight.length)}
      </>
    );
  }, [initialHighlight]);

  const processInitialHighlight = useCallback((children: ReactNode): ReactNode => {
    if (typeof children === 'string') return highlightInitialText(children);
    if (Array.isArray(children)) return children.map((child, i) => <span key={i}>{processInitialHighlight(child)}</span>);
    return children;
  }, [highlightInitialText]);

  const wrapWithHighlight = useCallback((children: ReactNode): ReactNode => {
    if (initialHighlight) return processInitialHighlight(children);
    if (isSearchOpen && searchQuery.trim()) return processChildren(children);
    return children;
  }, [isSearchOpen, searchQuery, processChildren, initialHighlight, processInitialHighlight]);

  const markdownComponents = useMemo(() => ({
    p: ({ children }: { children?: ReactNode }) => <p>{wrapWithHighlight(children)}</p>,
    li: ({ children }: { children?: ReactNode }) => <li>{wrapWithHighlight(children)}</li>,
    td: ({ children }: { children?: ReactNode }) => <td>{wrapWithHighlight(children)}</td>,
    th: ({ children }: { children?: ReactNode }) => <th>{wrapWithHighlight(children)}</th>,
    strong: ({ children }: { children?: ReactNode }) => <strong>{wrapWithHighlight(children)}</strong>,
    em: ({ children }: { children?: ReactNode }) => <em>{wrapWithHighlight(children)}</em>,
    del: ({ children }: { children?: ReactNode }) => <del>{wrapWithHighlight(children)}</del>,
    h1: ({ children }: { children?: ReactNode }) => <h1>{wrapWithHighlight(children)}</h1>,
    h2: ({ children }: { children?: ReactNode }) => <h2>{wrapWithHighlight(children)}</h2>,
    h3: ({ children }: { children?: ReactNode }) => <h3>{wrapWithHighlight(children)}</h3>,
    h4: ({ children }: { children?: ReactNode }) => <h4>{wrapWithHighlight(children)}</h4>,
    h5: ({ children }: { children?: ReactNode }) => <h5>{wrapWithHighlight(children)}</h5>,
    h6: ({ children }: { children?: ReactNode }) => <h6>{wrapWithHighlight(children)}</h6>,
    blockquote: ({ children }: { children?: ReactNode }) => <blockquote>{wrapWithHighlight(children)}</blockquote>,
    code: ({ className, children }: { className?: string; children?: ReactNode }) => {
      const isBlock = className?.startsWith('language-');
      if (isBlock) return <code className={className}>{wrapWithHighlight(children)}</code>;
      return <code>{wrapWithHighlight(children)}</code>;
    },
    pre: ({ children }: { children?: ReactNode }) => <pre>{children}</pre>,
    a: ({ href, children }: { href?: string; children?: ReactNode }) => {
      // If the link wraps an image, render the image unwrapped
      const childArray = Array.isArray(children) ? children : [children];
      if (childArray.some((c: any) => c?.type === MarkdownImage || c?.props?.src)) {
        return <>{children}</>;
      }
      return <LinkPreview url={href}>{wrapWithHighlight(children)}</LinkPreview>;
    },
    img: ({ src, alt }: { src?: string; alt?: string }) => <MarkdownImage src={src} alt={alt} />,
  }), [wrapWithHighlight]);

  // Click-to-edit: use caretRangeFromPoint to find the nearest text position,
  // then search for surrounding text in the raw markdown to estimate offset.
  const handleContentClick = useCallback((e: React.MouseEvent<HTMLElement>) => {
    if (isEditing) return;
    const target = e.target as HTMLElement;
    if (target.closest('a') || target.closest('button')) return;

    let offset = 0;
    const content = atom.content;
    if (content && document.caretRangeFromPoint) {
      const range = document.caretRangeFromPoint(e.clientX, e.clientY);
      if (range) {
        let textNode: Text | null = null;
        let charOffset = range.startOffset;

        if (range.startContainer.nodeType === Node.TEXT_NODE) {
          textNode = range.startContainer as Text;
        } else {
          // Element node — find the nearest text node from the child index
          const container = range.startContainer as Element;
          const childIndex = range.startOffset;
          // Try the child at the offset, then the one before it
          for (let i = childIndex; i >= Math.max(0, childIndex - 1); i--) {
            const child = container.childNodes[i];
            if (!child) continue;
            // Walk into the child to find its last/first text node
            const walker = document.createTreeWalker(child, NodeFilter.SHOW_TEXT);
            let last: Text | null = null;
            let node: Text | null;
            while ((node = walker.nextNode() as Text | null)) last = node;
            if (last) {
              textNode = last;
              charOffset = last.textContent?.length ?? 0; // end of text
              break;
            }
          }
        }

        if (textNode) {
          const text = textNode.textContent || '';
          const start = Math.max(0, charOffset - 10);
          const end = Math.min(text.length, charOffset + 10);
          const snippet = text.slice(start, end).trim();
          if (snippet) {
            // Find all occurrences of the snippet in the raw markdown
            const indices: number[] = [];
            let searchFrom = 0;
            while (searchFrom < content.length) {
              const idx = content.indexOf(snippet, searchFrom);
              if (idx === -1) break;
              indices.push(idx);
              searchFrom = idx + 1;
            }
            if (indices.length === 1) {
              offset = indices[0] + (charOffset - start);
            } else if (indices.length > 1) {
              // Pick the occurrence closest to the click's vertical position
              const article = articleRef.current;
              if (article) {
                const rect = article.getBoundingClientRect();
                const relativeY = (e.clientY - rect.top) / rect.height;
                const targetPos = relativeY * content.length;
                let bestIdx = indices[0];
                let bestDist = Math.abs(indices[0] - targetPos);
                for (const idx of indices) {
                  const dist = Math.abs(idx - targetPos);
                  if (dist < bestDist) {
                    bestDist = dist;
                    bestIdx = idx;
                  }
                }
                offset = bestIdx + (charOffset - start);
              } else {
                offset = indices[0] + (charOffset - start);
              }
            }
          }
        }
      }
    }
    startEditing(offset);
  }, [isEditing, atom.content, startEditing]);

  // Focus CodeMirror and set cursor position after mount
  // Poll briefly because the view may not be ready on the first frame
  useEffect(() => {
    if (!isEditing || cursorOffset === null) return;
    let attempts = 0;
    const tryFocus = () => {
      const view = editorRef.current?.view;
      if (view) {
        const pos = Math.min(cursorOffset, view.state.doc.length);
        view.focus();
        view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
      } else if (attempts < 10) {
        attempts++;
        requestAnimationFrame(tryFocus);
      }
    };
    requestAnimationFrame(tryFocus);
  }, [isEditing, cursorOffset]);

  const stopEditingRef = useRef(stopEditing);
  stopEditingRef.current = stopEditing;

  const editorExtensions = useMemo(() => getEditorExtensions(), []);

  // Document-level capture listener — fires before any element in the DOM tree
  useEffect(() => {
    if (!isEditing) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopImmediatePropagation();
        e.stopPropagation();
        e.preventDefault();
        stopEditingRef.current();
      }
    };
    document.addEventListener('keydown', handler, true);
    return () => document.removeEventListener('keydown', handler, true);
  }, [isEditing]);

  const handleDelete = async () => {
    setIsDeleting(true);
    try {
      await onDelete();
    } catch (error) {
      console.error('Failed to delete atom:', error);
    } finally {
      setIsDeleting(false);
      setShowDeleteModal(false);
    }
  };

  const proseClasses = `prose ${readerTheme === 'dark' ? 'prose-invert' : ''} max-w-none prose-headings:text-[var(--color-text-primary)] prose-p:text-[var(--color-text-primary)] prose-a:text-[var(--color-text-primary)] prose-a:underline prose-a:decoration-[var(--color-border-hover)] prose-a:hover:decoration-current prose-strong:text-[var(--color-text-primary)] prose-code:text-[var(--color-accent-light)] prose-code:bg-[var(--color-bg-card)] prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-pre:bg-[var(--color-bg-card)] prose-pre:border prose-pre:border-[var(--color-border)] prose-blockquote:border-l-[var(--color-accent)] prose-blockquote:text-[var(--color-text-secondary)] prose-li:text-[var(--color-text-primary)] prose-hr:border-[var(--color-border)]`;

  return (
    <div
      data-reader-theme={readerTheme}
      className={`h-full flex flex-col bg-[var(--color-bg-main)] transition-opacity duration-300 ease-out ${revealed ? 'opacity-100' : 'opacity-0'}`}
    >
      {/* Scrollable content area */}
      <div ref={scrollContainerRef} className="flex-1 overflow-y-auto scrollbar-auto-hide">
        {/* Search bar (only when not editing — CodeMirror has built-in search) */}
        {!isEditing && isSearchOpen && (
          <div className="max-w-2xl mx-auto px-6">
            <SearchBar
              query={searchQuery}
              searchedQuery={searchedQuery}
              onQueryChange={setSearchQuery}
              currentIndex={currentIndex}
              totalMatches={totalMatches}
              onNext={goToNext}
              onPrevious={goToPrevious}
              onClose={closeSearch}
            />
          </div>
        )}

        {/* Content area */}
        <div className="max-w-6xl mx-auto px-6 py-6 lg:flex lg:gap-10">
          {/* Prose column */}
          <div className={`flex-1 min-w-0 transition-[filter,opacity] duration-200 ${
            isTransitioning ? 'blur-[2px] opacity-60' : ''
          }`}>
            {isEditing ? (
              <div className={`max-w-3xl ${proseClasses}`}>
                <CodeMirror
                  ref={editorRef}
                  value={editContent}
                  onChange={setEditContent}
                  extensions={editorExtensions}
                  theme="none"
                  autoFocus
                  placeholder="Write your note in Markdown..."
                  className="min-h-[300px]"
                  basicSetup={{
                    lineNumbers: false,
                    highlightActiveLineGutter: false,
                    highlightActiveLine: false,
                    foldGutter: false,
                    bracketMatching: false,
                    closeBrackets: false,
                  }}
                />
              </div>
            ) : (
              <article
                ref={articleRef}
                className={`max-w-3xl cursor-text ${proseClasses}`}
                onClick={handleContentClick}
              >
                {chunks.slice(0, renderedChunkCount).map((chunk, index) => (
                  <MemoizedMarkdownChunk key={index} content={chunk} components={markdownComponents} />
                ))}

                <div className="h-8">
                  {!isFullyRendered && (
                    <div className="flex items-center gap-2 text-[var(--color-text-tertiary)]">
                      <Loader2 className="w-4 h-4 animate-spin" strokeWidth={2} />
                      <span className="text-sm">Loading...</span>
                    </div>
                  )}
                </div>
              </article>
            )}
          </div>

          {/* Metadata sidebar — right side on lg+ screens */}
          <div className="hidden lg:block w-80 shrink-0 border border-[var(--color-border)] rounded-lg p-4 self-start">
            {/* Source URL */}
            {isEditing ? (
              <div className="mb-4">
                <Input
                  value={editSourceUrl}
                  onChange={(e) => setEditSourceUrl(e.target.value)}
                  placeholder="Source URL (optional)"
                  className="text-xs"
                />
              </div>
            ) : atom.source_url ? (
              <div className="mb-4">
                <a
                  href={atom.source_url}
                  onClick={(e) => { e.preventDefault(); openExternalUrl(atom.source_url!).catch(console.error); }}
                  className="inline-block text-xs text-[var(--color-text-tertiary)] hover:text-[var(--color-accent)] bg-[var(--color-bg-card)] px-2 py-0.5 rounded-full cursor-pointer transition-colors"
                >
                  {atom.source || (() => { try { return new URL(atom.source_url!).hostname.replace(/^www\./, ''); } catch { return atom.source_url; } })()}
                </a>
              </div>
            ) : null}

            {/* Tags */}
            {isEditing ? (
              <div className="mb-4">
                <div className="flex flex-wrap gap-1.5 mb-2">
                  {editTags.map((tag) => (
                    <TagChip
                      key={tag.id}
                      name={tag.name}
                      size="sm"
                      onRemove={() => setEditTags(editTags.filter(t => t.id !== tag.id))}
                    />
                  ))}
                  <button
                    onClick={() => setShowTagSelector(!showTagSelector)}
                    className="text-xs text-[var(--color-accent)] hover:text-[var(--color-accent-light)] transition-colors px-1.5 py-0.5 rounded border border-dashed border-[var(--color-border)]"
                  >
                    +
                  </button>
                </div>
                {showTagSelector && (
                  <TagSelector selectedTags={editTags} onTagsChange={setEditTags} />
                )}
              </div>
            ) : atom.tags.length > 0 ? (
              <div className="flex flex-wrap gap-1.5 mb-4">
                {atom.tags.map((tag) => (
                  <TagChip
                    key={tag.id}
                    name={tag.name}
                    size="sm"
                    onClick={() => onTagClick(tag.id)}
                  />
                ))}
              </div>
            ) : null}

            {/* Dates */}
            <div className="text-xs text-[var(--color-text-tertiary)] space-y-0.5">
              {atom.published_at && <p>{formatDate(atom.published_at)}</p>}
              <p>{formatDate(atom.updated_at)}</p>
            </div>

            {/* Neighborhood graph — always visible */}
            {atom.embedding_status !== 'failed' && (
              <div className="mt-4">
                <MiniGraphPreview atomId={atom.id} onExpand={onViewGraph} />
              </div>
            )}

            {/* Related atoms — collapsible */}
            {atom.embedding_status !== 'failed' && (
              <SidebarRelatedAtoms atomId={atom.id} onAtomClick={onRelatedAtomClick} />
            )}
          </div>
        </div>
      </div>

      {/* Delete Confirmation Modal */}
      <Modal
        isOpen={showDeleteModal}
        onClose={() => setShowDeleteModal(false)}
        title="Delete Atom"
        confirmLabel={isDeleting ? 'Deleting...' : 'Delete'}
        confirmVariant="danger"
        onConfirm={handleDelete}
      >
        <p>Are you sure you want to delete this atom? This action cannot be undone.</p>
      </Modal>
    </div>
  );
}

function SidebarRelatedAtoms({ atomId, onAtomClick }: { atomId: string; onAtomClick: (id: string) => void }) {
  const [relatedAtoms, setRelatedAtoms] = useState<SimilarAtomResult[]>([]);
  const [isCollapsed, setIsCollapsed] = useState(true);
  const [hasLoaded, setHasLoaded] = useState(false);
  const [isLoading, setIsLoading] = useState(false);

  // Reset when atomId changes so we re-fetch for the new atom
  useEffect(() => {
    setRelatedAtoms([]);
    setHasLoaded(false);
  }, [atomId]);

  useEffect(() => {
    if (!isCollapsed && !hasLoaded) {
      setIsLoading(true);
      getTransport().invoke<SimilarAtomResult[]>('find_similar_atoms', { atomId, limit: 5, threshold: 0.7 })
        .then((results) => { setRelatedAtoms(results); setHasLoaded(true); })
        .catch(console.error)
        .finally(() => setIsLoading(false));
    }
  }, [atomId, isCollapsed, hasLoaded]);

  return (
    <div className="mt-4">
      <button
        onClick={() => setIsCollapsed(!isCollapsed)}
        className="flex items-center justify-between w-full text-xs font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors"
      >
        <span>Related atoms</span>
        <ChevronDown className={`w-3 h-3 transition-transform ${isCollapsed ? '' : 'rotate-180'}`} strokeWidth={2} />
      </button>
      {!isCollapsed && (
        <div className="mt-2 space-y-1.5">
          {isLoading ? (
            <div className="text-xs text-[var(--color-text-tertiary)]">Loading...</div>
          ) : relatedAtoms.length > 0 ? (
            relatedAtoms.map((result) => (
              <button
                key={result.id}
                onClick={() => onAtomClick(result.id)}
                className="w-full text-left p-2 rounded-md hover:bg-[var(--color-bg-hover)] transition-colors"
              >
                <p className="text-xs text-[var(--color-text-primary)] line-clamp-2">
                  {result.title || 'Untitled'}
                </p>
                <span className="text-[10px] text-[var(--color-accent)]">
                  {Math.round(result.similarity_score * 100)}% similar
                </span>
              </button>
            ))
          ) : hasLoaded ? (
            <div className="text-xs text-[var(--color-text-tertiary)]">No similar atoms found</div>
          ) : null}
        </div>
      )}
    </div>
  );
}

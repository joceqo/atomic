import { useEffect, useState, type ReactNode } from 'react';
import { getTransport } from '../../lib/transport';

interface LinkPreviewData {
  url: string;
  title?: string | null;
  description?: string | null;
  image?: string | null;
  site_name?: string | null;
}

interface LinkPreviewProps {
  url?: string;
  children?: ReactNode;
}

export function LinkPreview({ url, children }: LinkPreviewProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [data, setData] = useState<LinkPreviewData | null>(null);

  useEffect(() => {
    if (!isOpen || !url || data || loading) return;
    setLoading(true);
    getTransport().invoke<LinkPreviewData>('get_link_preview', { url })
      .then(setData)
      .catch(() => setData(null))
      .finally(() => setLoading(false));
  }, [isOpen, url, data, loading]);

  const displayTitle = data?.title?.trim() || url;
  const displayDescription = data?.description?.trim();
  const displaySite = data?.site_name?.trim();
  const previewImage = data?.image?.trim();
  const fallbackHost = (() => {
    if (!data?.url) return '';
    try {
      return new URL(data.url).hostname;
    } catch {
      return data.url;
    }
  })();

  return (
    <span
      className="relative inline-block"
      onMouseEnter={() => setIsOpen(true)}
      onMouseLeave={() => setIsOpen(false)}
      onFocus={() => setIsOpen(true)}
      onBlur={() => setIsOpen(false)}
    >
      {children}

      {isOpen && url && (
        <span className="absolute left-0 top-full mt-2 z-50 w-[320px] max-w-[80vw] rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-3 shadow-xl">
          {loading ? (
            <span className="block text-xs text-[var(--color-text-tertiary)]">Loading preview...</span>
          ) : data ? (
            <span className="block">
              {previewImage && (
                <img
                  src={previewImage}
                  alt=""
                  className="mb-2 h-36 w-full rounded object-cover border border-[var(--color-border)]"
                  loading="lazy"
                />
              )}
              <span className="block text-sm font-medium text-[var(--color-text-primary)] line-clamp-2">
                {displayTitle}
              </span>
              {displayDescription && (
                <span className="mt-1 block text-xs text-[var(--color-text-secondary)] line-clamp-3">
                  {displayDescription}
                </span>
              )}
              <span className="mt-2 block text-[11px] text-[var(--color-text-tertiary)]">
                {displaySite || fallbackHost}
              </span>
            </span>
          ) : (
            <span className="block text-xs text-[var(--color-text-tertiary)]">
              Preview unavailable
            </span>
          )}
        </span>
      )}
    </span>
  );
}

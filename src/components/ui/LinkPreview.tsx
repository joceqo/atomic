import { useEffect, useRef, useState, type ReactNode } from 'react';
import { getTransport } from '../../lib/transport';

interface LinkPreviewData {
  url: string;
  title?: string | null;
  description?: string | null;
  image?: string | null;
  site_name?: string | null;
}

interface ScreenshotEnqueueResponse {
  job_id: string;
  status: 'queued';
}

interface ScreenshotStatusResponse {
  job_id: string;
  status: 'pending' | 'processing' | 'completed' | 'failed';
  error?: string | null;
}

interface LinkPreviewProps {
  url?: string;
  children?: ReactNode;
}

export function LinkPreview({ url, children }: LinkPreviewProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [data, setData] = useState<LinkPreviewData | null>(null);
  const [screenshotJob, setScreenshotJob] = useState<ScreenshotStatusResponse | null>(null);
  const [screenshotImageUrl, setScreenshotImageUrl] = useState<string | null>(null);
  const queuedForUrl = useRef<string | null>(null);

  useEffect(() => {
    if (!isOpen || !url || data || loading) return;
    setLoading(true);
    getTransport().invoke<LinkPreviewData>('get_link_preview', { url })
      .then(setData)
      .catch(() => setData(null))
      .finally(() => setLoading(false));
  }, [isOpen, url, data, loading]);

  useEffect(() => {
    if (!isOpen || !url || !data || screenshotJob?.status === 'completed' || screenshotJob?.status === 'processing' || screenshotImageUrl || queuedForUrl.current === url) return;
    let cancelled = false;
    queuedForUrl.current = url;
    getTransport()
      .invoke<ScreenshotEnqueueResponse>('enqueue_link_screenshot', { url })
      .then((job) => {
        if (!cancelled) {
          setScreenshotJob({ job_id: job.job_id, status: 'pending' });
        }
      })
      .catch(() => {
        if (!cancelled) setScreenshotJob({ job_id: '', status: 'failed', error: 'Failed to queue screenshot' });
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen, url, data, screenshotJob?.status, screenshotImageUrl]);

  useEffect(() => {
    if (!isOpen) {
      queuedForUrl.current = null;
      setScreenshotJob(null);
      setScreenshotImageUrl(null);
    }
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen || !screenshotJob?.job_id || screenshotJob.status === 'completed' || screenshotJob.status === 'failed') return;
    const jobId = screenshotJob.job_id;
    const timer = setInterval(() => {
      getTransport()
        .invoke<ScreenshotStatusResponse>('get_link_screenshot_status', { jobId })
        .then((next) => {
          setScreenshotJob(next);
          if (next.status === 'completed') {
            clearInterval(timer);
            getTransport()
              .invoke<Blob>('get_link_screenshot_image', { jobId: next.job_id })
              .then((blob) => {
                const objectUrl = URL.createObjectURL(blob);
                setScreenshotImageUrl((prev) => {
                  if (prev) URL.revokeObjectURL(prev);
                  return objectUrl;
                });
              })
              .catch(() => setScreenshotJob((prev) => (prev ? { ...prev, status: 'failed', error: 'Failed to load screenshot' } : prev)));
          }
          if (next.status === 'failed') {
            clearInterval(timer);
          }
        })
        .catch(() => setScreenshotJob((prev) => (prev ? { ...prev, status: 'failed', error: 'Polling failed' } : prev)));
    }, 800);
    return () => clearInterval(timer);
  }, [isOpen, screenshotJob]);

  const displayTitle = data?.title?.trim() || url;
  const displayDescription = data?.description?.trim();
  const displaySite = data?.site_name?.trim();
  const previewImage = screenshotImageUrl || data?.image?.trim();
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

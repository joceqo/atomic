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
  status: string;
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

const linkPreviewDebug = import.meta.env.DEV
  ? (...args: unknown[]) => console.debug('[LinkPreview]', ...args)
  : () => {};

/** One enqueue per URL at a time — avoids N duplicate jobs when the same link appears in N markdown chunks. */
const screenshotEnqueueInflight = new Map<string, Promise<ScreenshotEnqueueResponse>>();

function enqueueScreenshotDeduped(url: string): Promise<ScreenshotEnqueueResponse> {
  const existing = screenshotEnqueueInflight.get(url);
  if (existing) {
    linkPreviewDebug('screenshot enqueue deduped (shared in-flight)', url);
    return existing;
  }
  const p = getTransport()
    .invoke<ScreenshotEnqueueResponse>('enqueue_link_screenshot', { url })
    .finally(() => {
      screenshotEnqueueInflight.delete(url);
    });
  screenshotEnqueueInflight.set(url, p);
  return p;
}

export function LinkPreview({ url, children }: LinkPreviewProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [data, setData] = useState<LinkPreviewData | null>(null);
  const [ogImageFailed, setOgImageFailed] = useState(false);
  const [screenshotJob, setScreenshotJob] = useState<ScreenshotStatusResponse | null>(null);
  const [screenshotImageUrl, setScreenshotImageUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen || !url || data || loading) return;
    setLoading(true);
    getTransport().invoke<LinkPreviewData>('get_link_preview', { url })
      .then((d) => {
        linkPreviewDebug('metadata', url, d);
        setData(d);
      })
      .catch((e) => {
        linkPreviewDebug('metadata failed', url, e);
        setData(null);
      })
      .finally(() => setLoading(false));
  }, [isOpen, url, data, loading]);

  useEffect(() => {
    if (!isOpen) {
      setOgImageFailed(false);
      setScreenshotJob(null);
      setScreenshotImageUrl(null);
    }
  }, [isOpen]);

  const needsScreenshot =
    Boolean(data) &&
    (!Boolean(data?.image?.trim()) || ogImageFailed) &&
    !screenshotImageUrl;

  useEffect(() => {
    if (!isOpen || !url || !data || !needsScreenshot) return;
    if (
      screenshotJob?.status === 'pending'
      || screenshotJob?.status === 'processing'
      || screenshotJob?.status === 'completed'
    ) {
      return;
    }

    let cancelled = false;
    enqueueScreenshotDeduped(url)
      .then((job) => {
        linkPreviewDebug('screenshot queued', url, job);
        if (!cancelled) {
          setScreenshotJob({ job_id: job.job_id, status: 'pending' });
        }
      })
      .catch((e) => {
        linkPreviewDebug('screenshot enqueue failed', url, e);
        if (!cancelled) setScreenshotJob({ job_id: '', status: 'failed', error: 'Failed to queue screenshot' });
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen, url, data, needsScreenshot, screenshotJob?.status, screenshotImageUrl]);

  const pollJobId = screenshotJob?.job_id;
  const pollTerminal = screenshotJob?.status === 'completed' || screenshotJob?.status === 'failed';

  useEffect(() => {
    if (!isOpen || !pollJobId || pollTerminal) return;

    const timer = setInterval(() => {
      getTransport()
        .invoke<ScreenshotStatusResponse>('get_link_screenshot_status', { jobId: pollJobId })
        .then((next) => {
          linkPreviewDebug('screenshot status', pollJobId, next);
          setScreenshotJob(next);
          if (next.status === 'completed') {
            clearInterval(timer);
            getTransport()
              .invoke<Blob>('get_link_screenshot_image', { jobId: next.job_id })
              .then((blob) => {
                linkPreviewDebug('screenshot image blob', { jobId: next.job_id, size: blob.size, type: blob.type });
                const objectUrl = URL.createObjectURL(blob);
                setScreenshotImageUrl((prev) => {
                  if (prev) URL.revokeObjectURL(prev);
                  return objectUrl;
                });
              })
              .catch((e) => {
                linkPreviewDebug('screenshot image fetch failed', next.job_id, e);
                setScreenshotJob((prev) => (prev ? { ...prev, status: 'failed', error: 'Failed to load screenshot' } : prev));
              });
          }
          if (next.status === 'failed') {
            clearInterval(timer);
            linkPreviewDebug('screenshot job failed', next.error);
          }
        })
        .catch(() => setScreenshotJob((prev) => (prev ? { ...prev, status: 'failed', error: 'Polling failed' } : prev)));
    }, 800);

    return () => clearInterval(timer);
  }, [isOpen, pollJobId, pollTerminal]);

  useEffect(() => {
    return () => {
      if (screenshotImageUrl) URL.revokeObjectURL(screenshotImageUrl);
    };
  }, [screenshotImageUrl]);

  const displayTitle = data?.title?.trim() || url;
  const displayDescription = data?.description?.trim();
  const displaySite = data?.site_name?.trim();
  const ogSrc = data?.image?.trim() && !ogImageFailed ? data.image.trim() : null;
  const previewImage = screenshotImageUrl || ogSrc;
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
                  onError={() => {
                    if (ogSrc && previewImage === ogSrc) {
                      linkPreviewDebug('og:image failed to load in browser, will try server screenshot', ogSrc);
                      setOgImageFailed(true);
                    }
                  }}
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

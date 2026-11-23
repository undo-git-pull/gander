import { useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/store";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

interface Reviewed {
  path: string;
  text: string;
  hash: string;
}

/**
 * Shown when the workspace's .gander/config.toml defines connections that are
 * not yet approved. Opening a connection runs its init_sql, extensions and
 * attaches, so a folder from elsewhere must not get that by default.
 */
export function TrustBanner() {
  const trust = useStore((s) => s.workspaceTrust);
  const [reviewed, setReviewed] = useState<Reviewed | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (!trust?.needs_trust) return null;

  const review = async () => {
    setError(null);
    try {
      setReviewed(await api.workspaceConfig());
    } catch (e) {
      setError(String(e));
    }
  };

  const approve = async () => {
    if (!reviewed) return;
    setBusy(true);
    try {
      await api.trustWorkspaceConfig(reviewed.hash);
      setReviewed(null);
    } catch (e) {
      // The file changed after it was shown: show the new contents instead.
      await review();
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="flex shrink-0 items-center gap-3 border-b bg-[var(--warning)]/10 px-3 py-1 text-xs">
        <span className="min-w-0 flex-1 truncate">
          This workspace's config defines connections, which can run SQL when opened. They stay off until you review and trust the file.
        </span>
        <Button size="xs" variant="outline" onClick={() => void review()}>
          Review
        </Button>
      </div>
      <Dialog open={reviewed != null} onOpenChange={(open) => !open && setReviewed(null)}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>Trust this workspace config?</DialogTitle>
            <DialogDescription className="break-all">{reviewed?.path}</DialogDescription>
          </DialogHeader>
          <p className="text-xs text-muted-foreground">
            Its connections can run any SQL when opened, including reading and writing files on this machine. Trust it only if you wrote it or know where it
            came from. Any later edit to the file needs approval again. Server settings in a workspace config are always ignored.
          </p>
          <pre className="max-h-[50vh] overflow-auto rounded-md border bg-muted p-2 font-mono text-xs whitespace-pre-wrap">{reviewed?.text}</pre>
          {error && <p className="text-xs text-destructive">{error}</p>}
          <DialogFooter>
            <Button size="sm" variant="outline" onClick={() => setReviewed(null)}>
              Cancel
            </Button>
            <Button size="sm" onClick={() => void approve()} disabled={busy}>
              Trust this file
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

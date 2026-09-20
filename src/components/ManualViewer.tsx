import { DocumentViewer, type DocumentKind } from "./DocumentViewer";

interface ManualViewerProps {
  path: string | null;
  kind: "pdf" | "txt" | "html" | null;
  open: boolean;
  onClose: () => void;
}

const KIND_LABEL: Record<DocumentKind, string> = { pdf: "PDF", txt: "Text", html: "HTML", image: "Image" };

export function ManualViewer(props: ManualViewerProps) {
  const filename = () => props.path?.split("/").pop() ?? "Manual";
  const source = () => (props.path && props.kind ? { kind: props.kind, path: props.path } : null);

  return (
    <DocumentViewer
      open={props.open}
      title={filename()}
      subtitle={props.kind ? `Manual · ${KIND_LABEL[props.kind]}` : "Manual"}
      source={source()}
      onClose={props.onClose}
      testId="manual-viewer"
    />
  );
}

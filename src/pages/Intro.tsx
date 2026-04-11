import { createSignal, For } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { WindowControls } from "../components/WindowControls";
import "./Intro.css";

interface Collection {
  id: string;
  name: string;
  description: string;
  games: string;
  torrent: string;
  languages: string[];
}

const COLLECTIONS: Collection[] = [
  {
    id: "eXoDOS",
    name: "eXoDOS",
    description: "The complete MS-DOS game collection with 7,667 pre-configured games.",
    games: "7,667 games",
    torrent: "eXoDOS.torrent",
    languages: ["EN"],
  },
  {
    id: "eXoDOS_GLP",
    name: "German Language Pack",
    description: "167 German-exclusive games plus 483 German translations of existing titles.",
    games: "650 games",
    torrent: "eXoDOS_GLP.torrent",
    languages: ["DE"],
  },
  {
    id: "eXoDOS_PLP",
    name: "Polish Language Pack",
    description: "238 Polish language games including exclusives and translations.",
    games: "238 games",
    torrent: "eXoDOS_PLP.torrent",
    languages: ["PL"],
  },
  {
    id: "eXoDOS_SLP",
    name: "Spanish Language Pack",
    description: "642 Spanish language games including exclusives and translations.",
    games: "642 games",
    torrent: "eXoDOS_SLP.torrent",
    languages: ["ES"],
  },
];

interface IntroProps {
  onSelect: (collections: string[]) => void;
}

export function Intro(props: IntroProps) {
  const [selected, setSelected] = createSignal<Set<string>>(new Set(["eXoDOS"]));

  const toggle = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        if (id !== "eXoDOS") next.delete(id); // eXoDOS is always required
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const handleContinue = () => {
    props.onSelect(Array.from(selected()));
  };

  return (
    <div class="intro-page" onMouseDown={(e) => {
      const tag = (e.target as HTMLElement).tagName.toLowerCase();
      if (['button', 'input', 'select', 'a', 'h1', 'h3', 'p', 'span', 'svg'].includes(tag)) return;
      if ((e.target as HTMLElement).closest('.collection-card, .intro-window-controls')) return;
      getCurrentWindow().startDragging();
    }}>
      <div class="intro-window-controls"><WindowControls /></div>
      <div class="intro-content">
        <h1 class="intro-title">Exodium</h1>
        <p class="intro-subtitle">Select the collections you want to set up</p>

        <div class="collection-grid">
          <For each={COLLECTIONS}>
            {(col) => (
              <div
                class={`collection-card ${selected().has(col.id) ? "selected" : ""} ${col.id === "eXoDOS" ? "required" : ""}`}
                onClick={() => toggle(col.id)}
              >
                <div class="collection-header">
                  <div class="collection-check">
                    {selected().has(col.id) ? "✓" : ""}
                  </div>
                  <h3>{col.name}</h3>
                </div>
                <p class="collection-desc">{col.description}</p>
                <div class="collection-meta">
                  <span>{col.games}</span>
                  <span class="collection-langs">
                    {col.languages.join(", ")}
                  </span>
                </div>
              </div>
            )}
          </For>
        </div>

        <button class="btn-primary intro-continue" onClick={handleContinue}>
          Continue
        </button>
      </div>
    </div>
  );
}

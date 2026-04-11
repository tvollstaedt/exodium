import { Show, onCleanup } from "solid-js";
import { setSearchQuery, fetchGames, searchQuery } from "../stores/games";

export function SearchBar() {
  let debounceTimer: ReturnType<typeof setTimeout>;
  onCleanup(() => clearTimeout(debounceTimer));

  const handleInput = (e: InputEvent) => {
    const value = (e.target as HTMLInputElement).value;
    setSearchQuery(value);
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => fetchGames(), 300);
  };

  const handleSubmit = (e: Event) => {
    e.preventDefault();
    clearTimeout(debounceTimer);
    fetchGames();
  };

  const handleClear = () => {
    setSearchQuery("");
    fetchGames();
  };

  return (
    <form class="search-bar" onSubmit={handleSubmit}>
      <input
        type="text"
        placeholder="Search games..."
        value={searchQuery()}
        onInput={handleInput}
      />
      <Show when={searchQuery()}>
        <button type="button" class="search-clear" onClick={handleClear}>×</button>
      </Show>
    </form>
  );
}

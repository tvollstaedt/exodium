import { setSearchQuery, fetchGames } from "../stores/games";

export function SearchBar() {
  const handleInput = (e: InputEvent) => {
    setSearchQuery((e.target as HTMLInputElement).value);
  };

  const handleSubmit = (e: Event) => {
    e.preventDefault();
    fetchGames();
  };

  return (
    <form class="search-bar" onSubmit={handleSubmit}>
      <input
        type="text"
        placeholder="Search games..."
        onInput={handleInput}
      />
    </form>
  );
}

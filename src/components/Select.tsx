import { For } from "solid-js";
import { Select as ArkSelect, createListCollection } from "@ark-ui/solid/select";
import { Portal } from "solid-js/web";

interface SelectOption {
  value: string;
  label: string;
}

interface SelectProps {
  options: SelectOption[];
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  class?: string;
}

export function Select(props: SelectProps) {
  const collection = () =>
    createListCollection({
      items: props.options,
      itemToValue: (item) => item.value,
      itemToString: (item) => item.label,
    });

  return (
    <ArkSelect.Root
      class={props.class}
      collection={collection()}
      value={props.value ? [props.value] : []}
      onValueChange={(details) => {
        const val = details.value[0] ?? "";
        props.onChange(val);
      }}
      positioning={{ sameWidth: true }}
    >
      <ArkSelect.Control>
        <ArkSelect.Trigger class="ark-select-trigger">
          <ArkSelect.ValueText placeholder={props.placeholder ?? "Select..."} />
          <span class="ark-select-arrow">&#9662;</span>
        </ArkSelect.Trigger>
      </ArkSelect.Control>
      <Portal>
        <ArkSelect.Positioner>
          <ArkSelect.Content class="ark-select-content">
            <For each={props.options}>
              {(option) => (
                <ArkSelect.Item item={option} class="ark-select-item">
                  <ArkSelect.ItemText>{option.label}</ArkSelect.ItemText>
                  <ArkSelect.ItemIndicator class="ark-select-indicator">
                    &#10003;
                  </ArkSelect.ItemIndicator>
                </ArkSelect.Item>
              )}
            </For>
          </ArkSelect.Content>
        </ArkSelect.Positioner>
      </Portal>
    </ArkSelect.Root>
  );
}

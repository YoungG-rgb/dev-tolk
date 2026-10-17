<script lang="ts">
  import { clock } from './format';
  import type { TranscriptData } from './types';

  let { data, onBack }: { data: TranscriptData; onBack: () => void } = $props();
</script>

<section class="transcript">
  <div class="bar">
    <button onclick={onBack}>← К списку</button>
    <h2>{data.title}</h2>
  </div>
  <p class="muted">
    Длительность {clock(data.durationMs)} · модель {data.model}{data.hasEdits ? ' · есть правки' : ''}. Просмотр без редактирования
    (редактирование появится позже).
  </p>
  {#if data.paragraphs.length === 0}
    <p class="muted">В записи не найдено речи.</p>
  {/if}
  {#each data.paragraphs as p}
    <p class="para"><span class="time">{clock(p.startMs)}</span> {p.text}</p>
  {/each}
</section>

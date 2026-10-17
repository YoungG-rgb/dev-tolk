<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';

  type AppInfo = {
    version: string;
    platform: string;
    arch: string;
    dataDir: string;
    logDir: string;
  };

  let info = $state<AppInfo | null>(null);
  let error = $state<string | null>(null);
  let csp = $state<'checking' | 'blocked' | 'not-blocked'>('checking');

  $effect(() => {
    invoke<AppInfo>('app_info')
      .then((value) => (info = value))
      .catch(() => (error = 'Не удалось получить сведения о приложении'));
  });

  // Самопроверка CSP: запрос на несуществующий домен (.invalid) должен быть заблокирован политикой
  // ещё до сети. Если политика не работает, запрос всё равно никуда не уйдёт: домен не разрешается.
  $effect(() => {
    if (csp !== 'checking') void invoke('report_csp_check', { blocked: csp === 'blocked' });
  });

  $effect(() => {
    const onViolation = (event: SecurityPolicyViolationEvent) => {
      if (event.violatedDirective.startsWith('connect-src')) csp = 'blocked';
    };
    document.addEventListener('securitypolicyviolation', onViolation);
    fetch('https://csp-check.invalid/').then(
      () => (csp = 'not-blocked'),
      () => {
        // Нет события нарушения за 1 с — блокировки политикой не было.
        setTimeout(() => {
          if (csp === 'checking') csp = 'not-blocked';
        }, 1000);
      },
    );
    return () => document.removeEventListener('securitypolicyviolation', onViolation);
  });
</script>

<main>
  <h1>dev-tolk</h1>
  <p class="muted">Локальная транскрибация записей. Каркас приложения: функций пока нет.</p>

  {#if error}
    <p role="alert">{error}</p>
  {:else if info}
    <dl>
      <dt>Версия</dt>
      <dd>{info.version}</dd>
      <dt>Система</dt>
      <dd>{info.platform} ({info.arch})</dd>
      <dt>Данные</dt>
      <dd>{info.dataDir}</dd>
      <dt>Журналы</dt>
      <dd>{info.logDir}</dd>
      <dt>Защита от сетевых запросов (CSP)</dt>
      <dd>
        {#if csp === 'checking'}проверяется…
        {:else if csp === 'blocked'}активна: внешний запрос заблокирован
        {:else}не сработала
        {/if}
      </dd>
    </dl>
  {/if}
</main>

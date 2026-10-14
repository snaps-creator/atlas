// Runs only inside the existing isolated CEF smoke harness, against synthetic
// native IPC snapshots. It never loads user settings or starts a VPN session.
(() => {
  const seen = [];
  let phase = 0, returning = false;
  const select = text => [...document.querySelectorAll('aside button')].find(b => b.textContent.trim() === text)?.click();
  const timer = setInterval(() => {
    if (!document.querySelector('main')) return;
    if (returning) {select('Серверы');returning=false;return;}
    select('Серверы');
    const cards=[...document.querySelectorAll('.server-card')];
    if (cards.length !== 3) return;
    const values=Object.fromEntries(cards.map(card=>[card.querySelector('strong').textContent,card.querySelector('.server-card-delay').textContent.replace(/\s/g,'')]));
    const delays=cards.map(card=>card.querySelector('.server-card-delay').textContent);
    if (delays.some(v=>/Устарело|выше порога|—|–|Ожидание|Проверяется|Неизвестно|Нет данных|Не проверен|Таймаут/.test(v))) {
      clearInterval(timer);throw Error('Forbidden ping label rendered');
    }
    const expected=[{'Low':'43мс','High':'1501мс','Failed':''},{'Low':'','High':'321мс','Failed':''},{'Low':'55мс','High':'','Failed':''},{'Low':'55мс','High':'321мс','Failed':''}];
    if (Object.entries(expected[phase]).every(([name,value])=>values[name]===value)) {
      seen.push(values);phase++;
      if (phase===expected.length) {
        clearInterval(timer);
        window.__TAURI_INTERNALS__.invoke('ui_smoke_ready',{title:document.title,buttons:document.querySelectorAll('button').length,latency:{phases:seen,navigations:3,forbiddenLabels:false}});
      } else {select('Главная');returning=true;}
    }
  }, 100);
})();

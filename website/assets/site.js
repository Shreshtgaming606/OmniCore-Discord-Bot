const menuButton = document.querySelector('.menu-toggle');
const menu = document.querySelector('#site-nav');
if (menuButton && menu) {
  const setMenu = open => {
    menuButton.setAttribute('aria-expanded', String(open));
    menuButton.setAttribute('aria-label', open ? 'Close navigation' : 'Open navigation');
    menu.classList.toggle('is-open', open);
  };
  menuButton.addEventListener('click', () => setMenu(menuButton.getAttribute('aria-expanded') !== 'true'));
  menu.addEventListener('click', event => { if (event.target.closest('a')) setMenu(false); });
  document.addEventListener('keydown', event => { if (event.key === 'Escape') setMenu(false); });
  document.addEventListener('click', event => { if (!menu.contains(event.target) && !menuButton.contains(event.target)) setMenu(false); });
}

const search = document.querySelector('#command-search');
const commandItems = [...document.querySelectorAll('.command-item')];
const filterButtons = [...document.querySelectorAll('.filter-button')];
const count = document.querySelector('#command-count');
const empty = document.querySelector('#command-empty');
if (search && commandItems.length && count && empty) {
  let category = 'All';
  const update = () => {
    const query = search.value.trim().toLowerCase();
    let visible = 0;
    for (const item of commandItems) {
      const matches = (category === 'All' || item.dataset.category === category) && (!query || item.dataset.search.includes(query));
      item.hidden = !matches;
      if (matches) visible++;
    }
    count.textContent = `${visible} command${visible === 1 ? '' : 's'} shown`;
    empty.hidden = visible !== 0;
  };
  search.addEventListener('input', update);
  for (const button of filterButtons) button.addEventListener('click', () => {
    category = button.dataset.filter;
    for (const other of filterButtons) {
      const active = other === button;
      other.classList.toggle('is-active', active);
      other.setAttribute('aria-pressed', String(active));
    }
    update();
  });
  const initial = new URLSearchParams(location.search).get('category');
  const initialButton = filterButtons.find(button => button.dataset.filter === initial);
  if (initialButton) initialButton.click();
  update();
}

if ('IntersectionObserver' in window && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
  const reveal = [...document.querySelectorAll('.overview-title, .overview-list, .spotlight-layout > div, .ai-copy, .transparency-layout > *, .feature-grid > *, .tool-columns > *, .support-grid > *')];
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) if (entry.isIntersecting) {
      entry.target.classList.add('is-visible');
      observer.unobserve(entry.target);
    }
  }, { threshold: .12, rootMargin: '0px 0px -35px 0px' });
  for (const item of reveal) { item.classList.add('will-reveal'); observer.observe(item); }
}

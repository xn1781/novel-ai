export function pagesBase(repository) {
  if (!repository) return '/';
  const [owner, name] = repository.split('/');
  if (!owner || !name) throw new Error('GITHUB_REPOSITORY must be owner/repository');
  return name.toLowerCase() === `${owner.toLowerCase()}.github.io` ? '/' : `/${name}/`;
}
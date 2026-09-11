export default function Aurora() {
  return (
    <div className="pointer-events-none absolute inset-0 -z-10 overflow-hidden" aria-hidden>
      <div className="absolute -top-[30%] -left-[10%] size-[70vw] animate-drift rounded-full bg-[radial-gradient(circle,rgba(65,222,23,0.12),transparent_62%)] blur-3xl" />
      <div className="absolute top-[20%] -right-[20%] size-[60vw] animate-drift-slow rounded-full bg-[radial-gradient(circle,rgba(226,140,60,0.07),transparent_62%)] blur-3xl" />
      <div className="absolute -bottom-[35%] left-[25%] size-[65vw] animate-drift-reverse rounded-full bg-[radial-gradient(circle,rgba(65,222,23,0.08),transparent_62%)] blur-3xl" />
    </div>
  );
}

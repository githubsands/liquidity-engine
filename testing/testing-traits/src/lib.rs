use crossbeam_channel::Receiver;

pub trait ProducerDefault<'a, T: Sized, U: 'a> {
    fn producer_default() -> (Box<Self>, Receiver<U>);
}

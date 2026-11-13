use super::*;
use Speaker::*;

#[test]
fn the_specifications_channel_orders_for_one_to_eight_channels() {
    assert_eq!(speakers(1), Some(&[FrontCenter][..]));
    assert_eq!(speakers(2), Some(&[FrontLeft, FrontRight][..]));
    assert_eq!(speakers(3), Some(&[FrontLeft, FrontCenter, FrontRight][..]));
    assert_eq!(
        speakers(4),
        Some(&[FrontLeft, FrontRight, RearLeft, RearRight][..])
    );
    assert_eq!(
        speakers(5),
        Some(&[FrontLeft, FrontCenter, FrontRight, RearLeft, RearRight][..])
    );
    assert_eq!(
        speakers(6),
        Some(&[FrontLeft, FrontCenter, FrontRight, RearLeft, RearRight, Lfe][..])
    );
    assert_eq!(
        speakers(7),
        Some(
            &[
                FrontLeft,
                FrontCenter,
                FrontRight,
                SideLeft,
                SideRight,
                RearCenter,
                Lfe
            ][..]
        )
    );
    assert_eq!(
        speakers(8),
        Some(
            &[
                FrontLeft,
                FrontCenter,
                FrontRight,
                SideLeft,
                SideRight,
                RearLeft,
                RearRight,
                Lfe
            ][..]
        )
    );
    assert_eq!(speakers(0), None);
    assert_eq!(speakers(9), None);
    assert_eq!(speakers(255), None);
}
